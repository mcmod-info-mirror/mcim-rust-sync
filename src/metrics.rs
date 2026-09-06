use std::convert::Infallible;
use std::fs;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use prometheus_client::encoding::text::encode;
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use tokio::net::TcpListener;

use crate::cli::{Command, CurseforgeTask, ModrinthTask};
use crate::task::TaskSummary;

#[derive(Clone, Debug, EncodeLabelSet, Hash, PartialEq, Eq)]
pub struct TaskLabels {
    pub task: String,
}

#[derive(Clone, Debug, EncodeLabelSet, Hash, PartialEq, Eq)]
pub struct TaskResultLabels {
    pub task: String,
    pub result: String,
}

#[derive(Clone, Debug, EncodeLabelSet, Hash, PartialEq, Eq)]
pub struct ItemLabels {
    pub task: String,
    pub provider: String,
    pub entity: String,
    pub operation: String,
    pub result: String,
}

#[derive(Clone)]
pub struct Metrics {
    registry: Arc<Mutex<Registry>>,
    pub task_runs: Family<TaskResultLabels, Counter>,
    pub task_duration: Family<TaskLabels, Histogram, fn() -> Histogram>,
    pub task_running: Family<TaskLabels, Gauge>,
    pub task_failures_streak: Family<TaskLabels, Gauge>,
    pub task_overlap_skips: Family<TaskLabels, Counter>,
    pub task_last_run_timestamp: Family<TaskLabels, Gauge>,
    pub task_last_success_timestamp: Family<TaskLabels, Gauge>,
    pub task_last_result: Family<TaskResultLabels, Gauge>,
    pub items: Family<ItemLabels, Counter>,
    pub process_resident_memory_bytes: Gauge,
    pub process_virtual_memory_bytes: Gauge,
    pub process_peak_resident_memory_bytes: Gauge,
    pub cgroup_memory_current_bytes: Gauge,
    pub cgroup_memory_peak_bytes: Gauge,
    pub cgroup_memory_limit_bytes: Gauge,
    started_at: Instant,
}

impl Metrics {
    pub fn new() -> Self {
        let mut registry = Registry::default();
        let task_runs = Family::default();
        let task_duration = Family::new_with_constructor(default_histogram as fn() -> Histogram);
        let task_running = Family::default();
        let task_failures_streak = Family::default();
        let task_overlap_skips = Family::default();
        let task_last_run_timestamp = Family::default();
        let task_last_success_timestamp = Family::default();
        let task_last_result = Family::default();
        let items = Family::default();
        let process_resident_memory_bytes = Gauge::default();
        let process_virtual_memory_bytes = Gauge::default();
        let process_peak_resident_memory_bytes = Gauge::default();
        let cgroup_memory_current_bytes = Gauge::default();
        let cgroup_memory_peak_bytes = Gauge::default();
        let cgroup_memory_limit_bytes = Gauge::default();

        registry.register("mcim_sync_task_runs", "Task runs", task_runs.clone());
        registry.register("mcim_sync_task_duration_seconds", "Task duration", task_duration.clone());
        registry.register("mcim_sync_task_running", "Running tasks", task_running.clone());
        registry.register("mcim_sync_task_failures_streak", "Consecutive task failures", task_failures_streak.clone());
        registry.register("mcim_sync_task_overlap_skips", "Overlapping task skips", task_overlap_skips.clone());
        registry.register("mcim_sync_task_last_run_timestamp_seconds", "Last task completion time", task_last_run_timestamp.clone());
        registry.register("mcim_sync_task_last_success_timestamp_seconds", "Last successful task completion time", task_last_success_timestamp.clone());
        registry.register("mcim_sync_task_last_result", "Last task result", task_last_result.clone());
        registry.register("mcim_sync_items", "Synchronized business items", items.clone());
        registry.register("mcim_sync_process_resident_memory_bytes", "Process resident memory", process_resident_memory_bytes.clone());
        registry.register("mcim_sync_process_virtual_memory_bytes", "Process virtual memory", process_virtual_memory_bytes.clone());
        registry.register("mcim_sync_process_peak_resident_memory_bytes", "Peak process resident memory", process_peak_resident_memory_bytes.clone());
        registry.register("mcim_sync_cgroup_memory_current_bytes", "Current cgroup memory", cgroup_memory_current_bytes.clone());
        registry.register("mcim_sync_cgroup_memory_peak_bytes", "Peak cgroup memory", cgroup_memory_peak_bytes.clone());
        registry.register("mcim_sync_cgroup_memory_limit_bytes", "Cgroup memory limit", cgroup_memory_limit_bytes.clone());

        Self {
            registry: Arc::new(Mutex::new(registry)),
            task_runs,
            task_duration,
            task_running,
            task_failures_streak,
            task_overlap_skips,
            task_last_run_timestamp,
            task_last_success_timestamp,
            task_last_result,
            items,
            process_resident_memory_bytes,
            process_virtual_memory_bytes,
            process_peak_resident_memory_bytes,
            cgroup_memory_current_bytes,
            cgroup_memory_peak_bytes,
            cgroup_memory_limit_bytes,
            started_at: Instant::now(),
        }
    }

    pub fn task(&self, name: &str) -> TaskLabels {
        TaskLabels { task: name.to_string() }
    }

    pub fn task_result(&self, name: &str, result: &str) -> TaskResultLabels {
        TaskResultLabels { task: name.to_string(), result: result.to_string() }
    }

    pub fn record_summary(&self, task_name: &str, command: &Command, summary: &TaskSummary) {
        let (provider, operation, entity) = match command {
            Command::Modrinth(task) => (
                "modrinth",
                match task {
                    ModrinthTask::Queue => "queue",
                    ModrinthTask::Refresh => "refresh",
                    ModrinthTask::RefreshFull => "refresh_full",
                    ModrinthTask::Search { .. } => "search",
                    ModrinthTask::Tags => "tags",
                },
                "project",
            ),
            Command::Curseforge(task) => (
                "curseforge",
                match task {
                    CurseforgeTask::Queue => "queue",
                    CurseforgeTask::Refresh => "refresh",
                    CurseforgeTask::Search { .. } => "search",
                    CurseforgeTask::Categories { .. } => "categories",
                },
                "mod",
            ),
            Command::Indexes | Command::Daemon => return,
        };

        if matches!(operation, "tags" | "categories") {
            return;
        }

        self.increment_items(task_name, provider, entity, operation, "attempted", summary.total);
        self.increment_items(task_name, provider, entity, operation, "synced", summary.synced);
        self.increment_items(task_name, provider, entity, operation, "not_found", summary.not_found);
        self.increment_items(task_name, provider, entity, operation, "skipped", summary.skipped);
        self.increment_items(task_name, provider, entity, operation, "failed", summary.failed);
        self.increment_items(task_name, provider, entity, operation, "requeued", summary.requeued);
        self.increment_items(task_name, provider, entity, operation, "discovered", summary.discovered);
        self.increment_items(task_name, provider, entity, operation, "removed", summary.removed);

        if provider == "modrinth" {
            self.increment_items(task_name, provider, "version", operation, "synced", summary.versions);
        }
        self.increment_items(task_name, provider, "file", operation, "synced", summary.files);
    }

    fn increment_items(&self, task: &str, provider: &str, entity: &str, operation: &str, result: &str, count: usize) {
        if count == 0 {
            return;
        }
        self.items
            .get_or_create(&ItemLabels {
                task: task.to_string(),
                provider: provider.to_string(),
                entity: entity.to_string(),
                operation: operation.to_string(),
                result: result.to_string(),
            })
            .inc_by(count as u64);
    }

    pub fn record_last_result(&self, name: &str, result: &str) {
        for known in ["success", "partial_failure", "error"] {
            self.task_last_result
                .get_or_create(&self.task_result(name, known))
                .set(i64::from(known == result));
        }
    }

    pub fn now_seconds() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64
    }

    pub fn refresh_memory(&self) {
        let status = fs::read_to_string("/proc/self/status").ok();
        if let Some(status) = status.as_deref() {
            let values = parse_proc_status(status);
            set_if_present(&self.process_resident_memory_bytes, values.rss_kb.map(kib_to_bytes));
            set_if_present(&self.process_virtual_memory_bytes, values.vmsize_kb.map(kib_to_bytes));
            set_if_present(&self.process_peak_resident_memory_bytes, values.hwm_kb.map(kib_to_bytes));
        }

        set_if_present(&self.cgroup_memory_current_bytes, read_u64("/sys/fs/cgroup/memory.current"));
        set_if_present(&self.cgroup_memory_peak_bytes, read_u64("/sys/fs/cgroup/memory.peak"));
        set_if_present(&self.cgroup_memory_limit_bytes, read_u64("/sys/fs/cgroup/memory.max"));

    }

    pub async fn serve(self: Arc<Self>, addr: SocketAddr) -> std::io::Result<()> {
        let listener = TcpListener::bind(addr).await?;
        tracing::info!(%addr, "metrics endpoint listening");
        loop {
            let (stream, _) = listener.accept().await?;
            let metrics = Arc::clone(&self);
            tokio::spawn(async move {
                let service = service_fn(move |request| metrics_response(Arc::clone(&metrics), request));
                if let Err(error) = http1::Builder::new().serve_connection(TokioIo::new(stream), service).await {
                    tracing::debug!(%error, "metrics connection closed");
                }
            });
        }
    }

    pub fn encode(&self) -> String {
        let mut output = String::new();
        let registry = self.registry.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        encode(&mut output, &registry).expect("encoding metrics should not fail");
        output.push_str("# HELP mcim_sync_uptime_seconds Process uptime\n# TYPE mcim_sync_uptime_seconds gauge\n");
        output.push_str(&format!("mcim_sync_uptime_seconds {}\n", self.started_at.elapsed().as_secs_f64()));
        output
    }
}

async fn metrics_response(
    metrics: Arc<Metrics>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    if request.uri().path() != "/metrics" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Full::new(Bytes::from_static(b"not found\n")))
            .unwrap());
    }
    Ok(Response::builder()
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(Full::new(Bytes::from(metrics.encode())))
        .unwrap())
}

fn default_histogram() -> Histogram {
    Histogram::new(vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 300.0, 900.0])
}

#[derive(Default)]
struct ProcStatus {
    rss_kb: Option<u64>,
    vmsize_kb: Option<u64>,
    hwm_kb: Option<u64>,
}

fn parse_proc_status(status: &str) -> ProcStatus {
    let mut result = ProcStatus::default();
    for line in status.lines() {
        let mut fields = line.split_whitespace();
        let value = fields.next().and_then(|_| fields.next()).and_then(|value| value.parse().ok());
        match line.split(':').next() {
            Some("VmRSS") => result.rss_kb = value,
            Some("VmSize") => result.vmsize_kb = value,
            Some("VmHWM") => result.hwm_kb = value,
            _ => {}
        }
    }
    result
}

fn read_u64(path: &str) -> Option<u64> {
    let value = fs::read_to_string(path).ok()?.trim().to_string();
    if value == "max" { None } else { value.parse().ok() }
}

fn set_if_present(gauge: &Gauge, value: Option<u64>) {
    if let Some(value) = value {
        gauge.set(value as i64);
    }
}

fn kib_to_bytes(value: u64) -> u64 {
    value.saturating_mul(1024)
}

#[cfg(test)]
mod tests {
    use super::parse_proc_status;

    #[test]
    fn parses_process_memory_fields() {
        let status = "VmSize: 123 kB\nVmRSS: 45 kB\nVmHWM: 67 kB\n";
        let parsed = parse_proc_status(status);
        assert_eq!(parsed.vmsize_kb, Some(123));
        assert_eq!(parsed.rss_kb, Some(45));
        assert_eq!(parsed.hwm_kb, Some(67));
    }
}
