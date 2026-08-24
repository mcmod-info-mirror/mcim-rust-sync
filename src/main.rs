use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use mcim_rust_sync::app::App;
use mcim_rust_sync::cli::{Cli, Command, CurseforgeTask, ModrinthTask, game_ids};
use mcim_rust_sync::config::Config;
use mcim_rust_sync::error::Result;
use mcim_rust_sync::task::{self, TaskSummary};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(&cli).await {
        Ok(summary) => {
            if summary.is_clean() {
                ExitCode::SUCCESS
            } else {
                // 有条目失败时以非零码退出，方便外部调度感知
                tracing::warn!(failed = summary.failed, "任务完成但存在失败条目");
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            tracing::error!(%error, "任务执行失败");
            ExitCode::FAILURE
        }
    }
}

fn init_tracing(verbose: bool) {
    let default = if verbose { "debug" } else { "info" };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("mcim_rust_sync={},warn", default)));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn run(cli: &Cli) -> Result<TaskSummary> {
    let config = Config::load(&cli.config)?;
    let app = App::new(config).await?;

    let summary = match &cli.command {
        Command::Curseforge(CurseforgeTask::Queue) => {
            let summary = task::curseforge::sync_queue(&app).await?;
            summary.log("curseforge queue");
            summary
        }
        Command::Curseforge(CurseforgeTask::Refresh) => {
            let summary = task::curseforge::refresh(&app).await?;
            summary.log("curseforge refresh");
            summary
        }
        Command::Curseforge(CurseforgeTask::RefreshFull) => {
            let summary = task::curseforge::refresh_full(&app).await?;
            summary.log("curseforge refresh-full");
            summary
        }
        Command::Curseforge(CurseforgeTask::Search { game_id }) => {
            let mut total = TaskSummary::default();
            for id in game_ids(*game_id) {
                let summary = task::curseforge::search(&app, id).await?;
                summary.log(&format!("curseforge search {}", id));
                merge(&mut total, summary);
            }
            total
        }
        Command::Curseforge(CurseforgeTask::Categories { game_id }) => {
            let mut total = TaskSummary::default();
            for id in game_ids(*game_id) {
                let summary = task::curseforge::categories(&app, id).await?;
                summary.log(&format!("curseforge categories {}", id));
                merge(&mut total, summary);
            }
            total
        }
        Command::Modrinth(ModrinthTask::Queue) => {
            let summary = task::modrinth::sync_queue(&app).await?;
            summary.log("modrinth queue");
            summary
        }
        Command::Modrinth(ModrinthTask::Refresh) => {
            let summary = task::modrinth::refresh(&app).await?;
            summary.log("modrinth refresh");
            summary
        }
        Command::Modrinth(ModrinthTask::RefreshFull) => {
            let summary = task::modrinth::refresh_full(&app).await?;
            summary.log("modrinth refresh-full");
            summary
        }
        Command::Modrinth(ModrinthTask::Search) => {
            let summary = task::modrinth::search(&app).await?;
            summary.log("modrinth search");
            summary
        }
        Command::Modrinth(ModrinthTask::Tags) => {
            let summary = task::modrinth::tags(&app).await?;
            summary.log("modrinth tags");
            summary
        }
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
