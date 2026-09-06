use crate::app::App;
use crate::cli::{Command, CurseforgeTask, ModrinthTask, game_ids};
use crate::error::{Error, Result};
use crate::models::task_run::{
    ERROR as RUN_ERROR, PARTIAL_FAILURE, SUCCESS, TaskRun, TaskRunStats,
};
use crate::task::{self, TaskSummary};

/// 执行一个任务，一次性模式与守护模式共用
pub async fn execute(app: &App, command: &Command) -> Result<TaskSummary> {
    let summary = match command {
        Command::Curseforge(CurseforgeTask::Queue) => {
            let summary = task::curseforge::sync_queue(app).await?;
            summary.log("curseforge queue");
            summary
        }
        Command::Curseforge(CurseforgeTask::Refresh) => {
            let summary = task::curseforge::refresh(app).await?;
            summary.log("curseforge refresh");
            summary
        }
        Command::Curseforge(CurseforgeTask::Search {
            game_id,
            max_pages,
            full,
        }) => {
            let mut total = TaskSummary::default();
            for id in game_ids(*game_id) {
                let summary = task::curseforge::search(app, id, *max_pages, *full).await?;
                summary.log(&format!("curseforge search {}", id));
                merge(&mut total, summary);
            }
            total
        }
        Command::Curseforge(CurseforgeTask::Categories { game_id }) => {
            let mut total = TaskSummary::default();
            for id in game_ids(*game_id) {
                let summary = task::curseforge::categories(app, id).await?;
                summary.log(&format!("curseforge categories {}", id));
                merge(&mut total, summary);
            }
            total
        }
        Command::Indexes => {
            let created = app.db.ensure_indexes().await?;
            for name in &created {
                tracing::info!(index = name, "索引就绪");
            }
            TaskSummary {
                total: created.len(),
                synced: created.len(),
                ..Default::default()
            }
        }
        Command::Modrinth(ModrinthTask::Queue) => {
            let summary = task::modrinth::sync_queue(app).await?;
            summary.log("modrinth queue");
            summary
        }
        Command::Modrinth(ModrinthTask::Refresh) => {
            let summary = task::modrinth::refresh(app).await?;
            summary.log("modrinth refresh");
            summary
        }
        Command::Modrinth(ModrinthTask::RefreshFull) => {
            let summary = task::modrinth::refresh_full(app).await?;
            summary.log("modrinth refresh-full");
            summary
        }
        Command::Modrinth(ModrinthTask::Search { max_pages, full }) => {
            let summary = task::modrinth::search(app, *max_pages, *full).await?;
            summary.log("modrinth search");
            summary
        }
        Command::Modrinth(ModrinthTask::Tags) => {
            let summary = task::modrinth::tags(app).await?;
            summary.log("modrinth tags");
            summary
        }
        // 由 main 分流到 daemon::run，走不到这里
        Command::Daemon => return Err(Error::Config("daemon 不是可执行的任务".to_string())),
    };

    Ok(summary)
}

/// 执行任务并持久化一条摘要记录；MongoDB 写入失败只告警，不影响同步结果。
pub async fn execute_with_history(
    app: &App,
    task_name: &str,
    command: &Command,
) -> Result<TaskSummary> {
    let started_at = chrono::Utc::now();
    let mut run = TaskRun::new(task_name, command, started_at);
    if let Some(run) = run.as_ref() {
        if let Err(error) = app.db.insert_task_run(run).await {
            tracing::warn!(task = %task_name, %error, "写入任务开始记录失败");
        }
    }

    let result = execute(app, command).await;
    let finished_at = chrono::Utc::now();
    if let Some(run) = run.as_mut() {
        match &result {
            Ok(summary) => run.finish(
                if summary.is_clean() {
                    SUCCESS
                } else {
                    PARTIAL_FAILURE
                },
                finished_at,
                stats(summary),
                None,
            ),
            Err(error) => run.finish(
                RUN_ERROR,
                finished_at,
                TaskRunStats::default(),
                Some(&error.to_string()),
            ),
        }
        if let Err(error) = app.db.update_task_run(run).await {
            tracing::warn!(task = %task_name, %error, "更新任务结束记录失败");
        }
    }

    result
}

pub fn command_name(command: &Command) -> &'static str {
    match command {
        Command::Curseforge(crate::cli::CurseforgeTask::Queue) => "curseforge-queue",
        Command::Curseforge(crate::cli::CurseforgeTask::Refresh) => "curseforge-refresh",
        Command::Curseforge(crate::cli::CurseforgeTask::Search { .. }) => "curseforge-search",
        Command::Curseforge(crate::cli::CurseforgeTask::Categories { .. }) => {
            "curseforge-categories"
        }
        Command::Modrinth(crate::cli::ModrinthTask::Queue) => "modrinth-queue",
        Command::Modrinth(crate::cli::ModrinthTask::Refresh) => "modrinth-refresh",
        Command::Modrinth(crate::cli::ModrinthTask::RefreshFull) => "modrinth-refresh-full",
        Command::Modrinth(crate::cli::ModrinthTask::Search { .. }) => "modrinth-search",
        Command::Modrinth(crate::cli::ModrinthTask::Tags) => "modrinth-tags",
        Command::Indexes => "indexes",
        Command::Daemon => "daemon",
    }
}

fn stats(summary: &TaskSummary) -> TaskRunStats {
    TaskRunStats {
        total: summary.total as i64,
        synced: summary.synced as i64,
        not_found: summary.not_found as i64,
        skipped: summary.skipped as i64,
        failed: summary.failed as i64,
        requeued: summary.requeued as i64,
        versions: summary.versions as i64,
        files: summary.files as i64,
        discovered: summary.discovered as i64,
        removed: summary.removed as i64,
    }
}

fn merge(total: &mut TaskSummary, other: TaskSummary) {
    total.total += other.total;
    total.synced += other.synced;
    total.not_found += other.not_found;
    total.skipped += other.skipped;
    total.failed += other.failed;
    total.requeued += other.requeued;
    total.versions += other.versions;
    total.files += other.files;
    total.discovered += other.discovered;
    total.removed += other.removed;
}
