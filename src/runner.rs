use crate::app::App;
use crate::cli::{Command, CurseforgeTask, ModrinthTask, game_ids};
use crate::error::{Error, Result};
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

fn merge(total: &mut TaskSummary, other: TaskSummary) {
    total.total += other.total;
    total.synced += other.synced;
    total.not_found += other.not_found;
    total.skipped += other.skipped;
    total.failed += other.failed;
    total.requeued += other.requeued;
}
