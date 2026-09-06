use bson::oid::ObjectId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::{Command, CurseforgeTask, ModrinthTask};

pub const RUNNING: &str = "running";
pub const SUCCESS: &str = "success";
pub const PARTIAL_FAILURE: &str = "partial_failure";
pub const ERROR: &str = "error";
pub const INTERRUPTED: &str = "interrupted";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRun {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub task: String,
    pub provider: String,
    pub operation: String,
    pub status: String,
    pub started_at: bson::DateTime,
    pub finished_at: Option<bson::DateTime>,
    pub duration_ms: Option<i64>,
    pub total: i64,
    pub synced: i64,
    pub not_found: i64,
    pub skipped: i64,
    pub failed: i64,
    pub requeued: i64,
    pub versions: i64,
    pub files: i64,
    pub discovered: i64,
    pub removed: i64,
    pub error: Option<String>,
    pub version: String,
}

#[derive(Debug, Clone, Default)]
pub struct TaskRunStats {
    pub total: i64,
    pub synced: i64,
    pub not_found: i64,
    pub skipped: i64,
    pub failed: i64,
    pub requeued: i64,
    pub versions: i64,
    pub files: i64,
    pub discovered: i64,
    pub removed: i64,
}

impl TaskRun {
    pub fn new(task: &str, command: &Command, started_at: DateTime<Utc>) -> Option<Self> {
        let (provider, operation) = match command {
            Command::Curseforge(task) => ("curseforge", curseforge_operation(task)),
            Command::Modrinth(task) => ("modrinth", modrinth_operation(task)),
            Command::Indexes | Command::Daemon => return None,
        };

        Some(Self {
            id: ObjectId::new(),
            task: task.to_string(),
            provider: provider.to_string(),
            operation: operation.to_string(),
            status: RUNNING.to_string(),
            started_at: bson::DateTime::from_chrono(started_at),
            finished_at: None,
            duration_ms: None,
            total: 0,
            synced: 0,
            not_found: 0,
            skipped: 0,
            failed: 0,
            requeued: 0,
            versions: 0,
            files: 0,
            discovered: 0,
            removed: 0,
            error: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
    }

    pub fn finish(
        &mut self,
        status: &str,
        finished_at: DateTime<Utc>,
        stats: TaskRunStats,
        error: Option<&str>,
    ) {
        self.status = status.to_string();
        self.finished_at = Some(bson::DateTime::from_chrono(finished_at));
        self.duration_ms = Some(
            (finished_at - self.started_at.to_chrono())
                .num_milliseconds()
                .max(0),
        );
        self.total = stats.total;
        self.synced = stats.synced;
        self.not_found = stats.not_found;
        self.skipped = stats.skipped;
        self.failed = stats.failed;
        self.requeued = stats.requeued;
        self.versions = stats.versions;
        self.files = stats.files;
        self.discovered = stats.discovered;
        self.removed = stats.removed;
        self.error = error.map(|value| value.chars().take(1024).collect());
    }
}

fn curseforge_operation(task: &CurseforgeTask) -> &'static str {
    match task {
        CurseforgeTask::Queue => "queue",
        CurseforgeTask::Refresh => "refresh",
        CurseforgeTask::Search { .. } => "search",
        CurseforgeTask::Categories { .. } => "categories",
    }
}

fn modrinth_operation(task: &ModrinthTask) -> &'static str {
    match task {
        ModrinthTask::Queue => "queue",
        ModrinthTask::Refresh => "refresh",
        ModrinthTask::RefreshFull => "refresh-full",
        ModrinthTask::Search { .. } => "search",
        ModrinthTask::Tags => "tags",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_run_serializes_with_bson_dates() {
        let command = Command::Modrinth(ModrinthTask::Refresh);
        let run = TaskRun::new("nightly-refresh", &command, Utc::now()).expect("task run");
        let document = bson::serialize_to_document(&run).expect("bson document");
        assert_eq!(document.get_str("task").unwrap(), "nightly-refresh");
        assert!(document.get_datetime("started_at").is_ok());
        assert!(serde_json::to_value(&run).is_ok());
    }

    #[test]
    fn indexes_are_not_recorded_as_sync_task_runs() {
        assert!(TaskRun::new("indexes", &Command::Indexes, Utc::now()).is_none());
    }
}
