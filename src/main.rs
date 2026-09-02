use std::process::ExitCode;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use mcim_rust_sync::app::App;
use mcim_rust_sync::cli::{Cli, Command};
use mcim_rust_sync::config::Config;
use mcim_rust_sync::error::Result;
use mcim_rust_sync::runner::execute;
use mcim_rust_sync::{daemon, task::TaskSummary};

/// 跑完了，但有个别条目没同步成功
const EXIT_COMPLETED_WITH_FAILURES: u8 = 1;
/// 任务整体失败，没跑完
const EXIT_ERROR: u8 = 2;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    // 守护模式常驻，没有「跑完」这回事，退出码只区分正常停止与出错
    if matches!(cli.command, Command::Daemon) {
        return match run_daemon(&cli).await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(%error, "守护进程退出");
                ExitCode::from(EXIT_ERROR)
            }
        };
    }

    match run(&cli).await {
        Ok(summary) => {
            if summary.is_clean() {
                ExitCode::SUCCESS
            } else {
                // 跑完了但有个别条目失败，与整体出错要分开，
                // 否则外部看护脚本无法判断该重试还是该收工
                tracing::warn!(failed = summary.failed, "任务完成但存在失败条目");
                ExitCode::from(EXIT_COMPLETED_WITH_FAILURES)
            }
        }
        Err(error) => {
            tracing::error!(%error, "任务执行失败");
            ExitCode::from(EXIT_ERROR)
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
    execute(&app, &cli.command).await
}

async fn run_daemon(cli: &Cli) -> Result<()> {
    let config = Config::load(&cli.config)?;
    let app = App::new(config).await?;
    daemon::run(app).await
}
