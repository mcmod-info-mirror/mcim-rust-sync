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
        let capability = unsafe {
            (
                tikv_jemalloc_ctl::raw::read::<bool>(b"config.prof\0"),
                tikv_jemalloc_ctl::raw::read::<bool>(b"opt.prof\0"),
                tikv_jemalloc_ctl::raw::read::<bool>(b"opt.prof_active\0"),
                tikv_jemalloc_ctl::raw::read::<bool>(b"prof.active\0"),
            )
        };
        tracing::info!(?capability, "jemalloc profiling capability");
        format!(
            "allocated={} active={} resident={}",
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

pub async fn dump(label: &str, project_id: Option<&str>) {
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
        let Some(prof_ctl) = jemalloc_pprof::PROF_CTL.as_ref() else {
            tracing::warn!("jemalloc profiling is not available in this binary");
            return;
        };

        let mut prof_ctl = prof_ctl.lock().await;
        if !prof_ctl.activated() {
            tracing::warn!("jemalloc profiling is not activated");
            return;
        }

        let pprof = match prof_ctl.dump_pprof() {
            Ok(pprof) => pprof,
            Err(error) => {
                tracing::warn!(%error, "jemalloc pprof dump failed");
                return;
            }
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default();
        let safe_label = label.replace(|ch: char| !ch.is_ascii_alphanumeric() && ch != '-', "_");
        let path = dir.join(format!("{stamp}-{safe_label}.pb.gz"));
        match fs::write(&path, pprof) {
            Ok(()) => tracing::info!(path = %path.display(), "jemalloc pprof written"),
            Err(error) => tracing::warn!(%error, ?path, "failed to write jemalloc pprof"),
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
