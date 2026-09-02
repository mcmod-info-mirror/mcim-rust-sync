use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use croner::Cron;
use tokio::task::JoinSet;

use crate::app::App;
use crate::cli::Command;
use crate::config::ScheduleEntry;
use crate::error::{Error, Result};
use crate::runner::execute;

/// 单次睡眠上限，系统时钟被调整后不至于一直睡在旧的到期时刻上
const MAX_SLEEP: Duration = Duration::from_secs(60);

/// 一条排好期的计划
struct Job {
    name: String,
    cron: Cron,
    command: Command,

    /// Some(起始时刻) 表示这个任务正在跑
    slot: Arc<Mutex<Option<Instant>>>,

    /// 连续失败次数，守护模式下没有退出码可看，持续故障靠它叫出来
    failures: Arc<AtomicU32>,

    next: DateTime<Utc>,
}

/// 常驻，按 schedule 定时执行任务
pub async fn run(app: App) -> Result<()> {
    let app = Arc::new(app);
    let mut jobs = build(&app.config.schedule, Utc::now())?;
    if jobs.is_empty() {
        return Err(Error::Config("schedule 是空的，没有任务可排".to_string()));
    }

    // 索引幂等，启动时一次建好，免得某次任务跑到一半才触发首次构建
    for name in app.db.ensure_indexes().await? {
        tracing::info!(index = %name, "索引就绪");
    }
    for job in &jobs {
        tracing::info!(task = %job.name, next = %job.next, "已排期");
    }

    let mut running = JoinSet::new();
    let mut stop = Box::pin(wait_for_stop());

    loop {
        let delay = until_next(&jobs, Utc::now());
        tokio::select! {
            _ = tokio::time::sleep(delay) => {
                spawn_due(&app, &mut jobs, &mut running, Utc::now());
            }
            Some(result) = running.join_next(), if !running.is_empty() => {
                if let Err(error) = result {
                    tracing::error!(%error, "任务线程异常退出");
                }
            }
            signal = &mut stop => {
                tracing::info!(signal = signal?, "收到停止信号，不再排新任务");
                break;
            }
        }
    }

    shutdown(running, app.config.shutdown_grace_secs).await;
    Ok(())
}

/// 把配置里的计划解析成任务，任何一条不合法都直接失败而不是跳过
fn build(schedule: &BTreeMap<String, ScheduleEntry>, now: DateTime<Utc>) -> Result<Vec<Job>> {
    let mut jobs = Vec::with_capacity(schedule.len());
    for (name, entry) in schedule {
        let command = Command::parse_args(&entry.args)
            .map_err(|e| Error::Config(format!("schedule.{} 的 args 无效: {}", name, e)))?;
        let cron: Cron = entry
            .cron
            .parse()
            .map_err(|e| Error::Config(format!("schedule.{} 的 cron 无效: {}", name, e)))?;
        let next = next_after(&cron, now)
            .ok_or_else(|| Error::Config(format!("schedule.{} 的 cron 永远不会触发", name)))?;

        jobs.push(Job {
            name: name.clone(),
            cron,
            command,
            slot: Arc::new(Mutex::new(None)),
            failures: Arc::new(AtomicU32::new(0)),
            next,
        });
    }
    Ok(jobs)
}

/// 下一个触发时刻
fn next_after(cron: &Cron, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
    cron.find_next_occurrence(&after, false).ok()
}

/// 距离最早一条计划到期还有多久
fn until_next(jobs: &[Job], now: DateTime<Utc>) -> Duration {
    jobs.iter()
        .map(|job| job.next)
        .min()
        .and_then(|next| (next - now).to_std().ok())
        .unwrap_or(Duration::ZERO)
        .min(MAX_SLEEP)
}

/// 派发所有到期的任务
fn spawn_due(app: &Arc<App>, jobs: &mut [Job], running: &mut JoinSet<()>, now: DateTime<Utc>) {
    for job in jobs.iter_mut() {
        if job.next > now {
            continue;
        }

        // 不补跑，停机期间错过的班次直接跳到下一班，任务本身都是幂等增量
        match next_after(&job.cron, now) {
            Some(next) => job.next = next,
            None => {
                tracing::error!(task = %job.name, "算不出下次触发时刻，这条计划停掉");
                job.next = DateTime::<Utc>::MAX_UTC;
            }
        }

        let claim = match claim(&job.slot) {
            Ok(claim) => claim,
            Err(elapsed) => {
                tracing::warn!(task = %job.name, ?elapsed, "上一轮还没跑完，跳过本轮");
                continue;
            }
        };

        let app = Arc::clone(app);
        let command = job.command.clone();
        let name = job.name.clone();
        let failures = Arc::clone(&job.failures);

        running.spawn(async move {
            let _claim = claim;
            let started = Instant::now();
            tracing::info!(task = %name, "本轮开始");

            match execute(&app, &command).await {
                Ok(summary) => {
                    failures.store(0, Ordering::Relaxed);
                    if summary.is_clean() {
                        tracing::info!(task = %name, elapsed = ?started.elapsed(), "本轮结束");
                    } else {
                        tracing::warn!(
                            task = %name,
                            failed = summary.failed,
                            elapsed = ?started.elapsed(),
                            "本轮结束，有条目没同步成功"
                        );
                    }
                }
                Err(error) => {
                    let streak = failures.fetch_add(1, Ordering::Relaxed) + 1;
                    tracing::error!(
                        task = %name,
                        %error,
                        streak,
                        elapsed = ?started.elapsed(),
                        "本轮失败"
                    );
                }
            }
        });
    }
}

/// 任务结束时清掉占位，panic 也能清掉，否则这个任务再也排不上
struct Claim(Arc<Mutex<Option<Instant>>>);

impl Drop for Claim {
    fn drop(&mut self) {
        *lock(&self.0) = None;
    }
}

fn lock(slot: &Mutex<Option<Instant>>) -> MutexGuard<'_, Option<Instant>> {
    slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 占住任务，已经在跑就返回它已经跑了多久
///
/// 只挡同一个任务自我重叠，不同任务照常并发，对上游的总速率由
/// HttpClient 里按域名共享的令牌桶约束
fn claim(slot: &Arc<Mutex<Option<Instant>>>) -> std::result::Result<Claim, Duration> {
    let mut guard = lock(slot);
    if let Some(since) = *guard {
        return Err(since.elapsed());
    }
    *guard = Some(Instant::now());
    drop(guard);
    Ok(Claim(Arc::clone(slot)))
}

/// SIGTERM 与 SIGINT 都当成停止信号
async fn wait_for_stop() -> std::io::Result<&'static str> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut term = signal(SignalKind::terminate())?;
        let mut interrupt = signal(SignalKind::interrupt())?;
        tokio::select! {
            _ = term.recv() => Ok("SIGTERM"),
            _ = interrupt.recv() => Ok("SIGINT"),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await?;
        Ok("Ctrl-C")
    }
}

/// 等在跑的任务收尾，超时就强行终止
///
/// 只有 queue 任务会 SPOP 取走队列成员，它是分钟级的，正常都能在宽限期内跑完；
/// 真被强行终止的只会是 refresh 与 search 这类幂等任务，下一轮重跑即可
async fn shutdown(mut running: JoinSet<()>, grace_secs: u64) {
    if running.is_empty() {
        return;
    }

    let grace = Duration::from_secs(grace_secs);
    tracing::info!(count = running.len(), ?grace, "等待在跑的任务收尾");

    let finished = tokio::time::timeout(grace, async {
        while running.join_next().await.is_some() {}
    })
    .await;

    if finished.is_err() {
        tracing::error!(count = running.len(), "宽限期内没跑完，强行终止");
        running.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{CurseforgeTask, ModrinthTask};

    fn schedule(pairs: &[(&str, &str, &str)]) -> BTreeMap<String, ScheduleEntry> {
        pairs
            .iter()
            .map(|(name, cron, args)| {
                (
                    name.to_string(),
                    ScheduleEntry {
                        cron: cron.to_string(),
                        args: args.to_string(),
                    },
                )
            })
            .collect()
    }

    fn at(value: &str) -> DateTime<Utc> {
        value.parse().expect("时间解析失败")
    }

    #[test]
    fn builds_jobs_with_next_fire_time() {
        let jobs = build(
            &schedule(&[
                ("curseforge-queue", "*/5 * * * *", "curseforge queue"),
                ("modrinth-tags", "0 0 * * *", "modrinth tags"),
            ]),
            at("2026-09-02T10:01:00Z"),
        )
        .expect("解析计划失败");

        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].name, "curseforge-queue");
        assert_eq!(jobs[0].next, at("2026-09-02T10:05:00Z"));
        assert_eq!(jobs[1].next, at("2026-09-03T00:00:00Z"));
    }

    #[test]
    fn parses_task_arguments() {
        let jobs = build(
            &schedule(&[(
                "curseforge-search",
                "0 * * * *",
                "curseforge search --game-id 432 --max-pages 20",
            )]),
            at("2026-09-02T10:01:00Z"),
        )
        .expect("解析计划失败");

        match &jobs[0].command {
            Command::Curseforge(CurseforgeTask::Search {
                game_id,
                max_pages,
                full,
            }) => {
                assert_eq!(*game_id, Some(432));
                assert_eq!(*max_pages, 20);
                assert!(!*full);
            }
            other => panic!("解析出了别的任务: {:?}", other),
        }
    }

    #[test]
    fn defaults_come_from_the_same_clap_definition() {
        let jobs = build(
            &schedule(&[("modrinth-search", "0 * * * *", "modrinth search")]),
            at("2026-09-02T10:01:00Z"),
        )
        .expect("解析计划失败");

        match &jobs[0].command {
            Command::Modrinth(ModrinthTask::Search { max_pages, full }) => {
                assert_eq!(*max_pages, 0);
                assert!(!*full);
            }
            other => panic!("解析出了别的任务: {:?}", other),
        }
    }

    /// 一条计划写错就整个起不来，不能默默跳过让人以为在跑
    #[test]
    fn invalid_cron_is_an_error() {
        let result = build(
            &schedule(&[("坏计划", "每两小时", "modrinth tags")]),
            at("2026-09-02T10:01:00Z"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn unknown_task_is_an_error() {
        let result = build(
            &schedule(&[("坏计划", "0 * * * *", "modrinth 不存在的任务")]),
            at("2026-09-02T10:01:00Z"),
        );
        assert!(result.is_err());
    }

    /// 守护进程套守护进程会无限递归
    #[test]
    fn daemon_cannot_be_scheduled() {
        let result = build(
            &schedule(&[("套娃", "0 * * * *", "daemon")]),
            at("2026-09-02T10:01:00Z"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn sleep_is_capped() {
        let jobs = build(
            &schedule(&[("modrinth-tags", "0 0 * * *", "modrinth tags")]),
            at("2026-09-02T10:01:00Z"),
        )
        .expect("解析计划失败");

        // 下一班在十几个小时以后，但单次最多睡 MAX_SLEEP
        assert_eq!(until_next(&jobs, at("2026-09-02T10:01:00Z")), MAX_SLEEP);
    }

    #[test]
    fn overdue_job_does_not_wait() {
        let mut jobs = build(
            &schedule(&[("modrinth-tags", "0 0 * * *", "modrinth tags")]),
            at("2026-09-02T10:01:00Z"),
        )
        .expect("解析计划失败");
        jobs[0].next = at("2026-09-02T09:00:00Z");

        assert_eq!(
            until_next(&jobs, at("2026-09-02T10:01:00Z")),
            Duration::ZERO
        );
    }

    #[test]
    fn running_job_cannot_be_claimed_twice() {
        let slot = Arc::new(Mutex::new(None));
        let first = claim(&slot).expect("第一次应该占得住");
        assert!(claim(&slot).is_err(), "同一个任务不应该重叠");

        drop(first);
        assert!(claim(&slot).is_ok(), "上一轮结束后应该能重新排上");
    }

    /// 任务 panic 也要把占位清掉，否则它再也排不上
    #[test]
    fn claim_is_released_on_unwind() {
        let slot = Arc::new(Mutex::new(None));
        let panicked = std::panic::catch_unwind({
            let slot = Arc::clone(&slot);
            move || {
                let _claim = claim(&slot).expect("应该占得住");
                panic!("任务炸了");
            }
        });

        assert!(panicked.is_err());
        assert!(claim(&slot).is_ok(), "panic 后占位没被清掉");
    }
}
