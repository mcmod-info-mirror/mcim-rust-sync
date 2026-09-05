use std::ffi::CString;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(target_env = "msvc"))]
use tikv_jemalloc_ctl::{epoch, stats};

pub fn enabled() -> bool {
    std::env::var_os("MCIM_PROFILE_DIR").is_some()
}

pub fn project_enabled(project_id: &str) -> bool {
    std::env::var("MCIM_PROFILE_PROJECT_ID")
        .map(|value| value.is_empty() || value == project_id)
        .unwrap_or(false)
}

pub fn snapshot(label: &str, project_id: Option<&str>) {
    if !enabled() {
        return;
    }

    let (rss_kb, hwm_kb) = proc_memory();
    #[cfg(not(target_env = "msvc"))]
    let jemalloc = {
        let _ = epoch::advance();
        let profiling = [
            (
                "config.prof",
                tikv_jemalloc_ctl::raw::read::<bool>(b"config.prof\0"),
            ),
            (
                "opt.prof",
                tikv_jemalloc_ctl::raw::read::<bool>(b"opt.prof\0"),
            ),
            (
                "prof.active",
                tikv_jemalloc_ctl::raw::read::<bool>(b"prof.active\0"),
            ),
        ];
        tracing::debug!(?profiling, "jemalloc profiling capability");
        format!(
            " allocated={} active={} resident={}",
            stats::allocated::read().unwrap_or_default(),
            stats::active::read().unwrap_or_default(),
            stats::resident::read().unwrap_or_default()
        )
    };
    #[cfg(target_env = "msvc")]
    let jemalloc = String::new();

    tracing::info!(
        label,
        project_id = ?project_id,
        rss_kb,
        hwm_kb,
        jemalloc = %jemalloc,
        "profile memory snapshot"
    );
}

pub fn dump(label: &str, project_id: Option<&str>) {
    if !enabled() {
        return;
    }

    let dir = PathBuf::from(std::env::var_os("MCIM_PROFILE_DIR").expect("checked above"));
    if let Err(error) = fs::create_dir_all(&dir) {
        tracing::warn!(%error, ?dir, "failed to create profile directory");
        return;
    }

    snapshot(label, project_id);
    #[cfg(not(target_env = "msvc"))]
    {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let safe_label = label.replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-', "_");
        let name = format!("{stamp}-{safe_label}.heap");
        let path = dir.join(name);
        let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) else {
            return;
        };
        let result = unsafe { tikv_jemalloc_ctl::raw::write(b"prof.dump\0", path.as_ptr()) };
        if let Err(error) = result {
            tracing::warn!(%error, "jemalloc heap dump failed");
        } else {
            tracing::info!(path = %path.to_string_lossy(), "jemalloc heap dump written");
        }
    }
}

fn proc_memory() -> (Option<u64>, Option<u64>) {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return (None, None);
    };
    let mut rss = None;
    let mut hwm = None;
    for line in status.lines() {
        let mut fields = line.split_whitespace();
        match fields.next() {
            Some("VmRSS:") => rss = fields.next().and_then(|value| value.parse().ok()),
            Some("VmHWM:") => hwm = fields.next().and_then(|value| value.parse().ok()),
            _ => {}
        }
    }
    (rss, hwm)
}
