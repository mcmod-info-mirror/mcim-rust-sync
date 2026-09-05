use std::collections::{BTreeSet, HashMap, HashSet};

use bson::doc;
use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde::Deserialize;
use serde_with::serde_as;

use crate::app::App;
use crate::constants::MODRINTH_SEARCH_PAGE_SIZE;
use crate::db::queue::key;
use crate::error::Result;
use crate::models::collection;
use crate::models::{FlexDateTime, modrinth::Project};
use crate::sync::modrinth::ModrinthSync;

use super::{TaskSummary, requeue, same_second};

#[serde_as]
#[derive(Debug, Deserialize)]
struct ProjectStamp {
    #[serde(rename = "_id")]
    id: String,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default)]
    updated: Option<DateTime<Utc>>,
    #[serde(default)]
    versions: Option<Vec<String>>,
    #[serde(default)]
    game_versions: Option<Vec<String>>,
}

fn same_set(left: Option<&Vec<String>>, right: Option<&Vec<String>>) -> bool {
    // 按集合比较，Python 版按顺序比较，上游重排就会触发一次无意义的重同步
    let left: HashSet<&String> = left.into_iter().flatten().collect();
    let right: HashSet<&String> = right.into_iter().flatten().collect();
    left == right
}

fn is_outdated(local: &ProjectStamp, remote: &Project) -> bool {
    if !same_second(local.updated, Some(remote.updated)) {
        return true;
    }
    if !same_set(local.versions.as_ref(), remote.versions.as_ref()) {
        return true;
    }
    !same_set(local.game_versions.as_ref(), remote.game_versions.as_ref())
}

fn summarize(
    report: &crate::sync::Report<String, crate::sync::modrinth::ProjectSummary>,
) -> TaskSummary {
    TaskSummary {
        total: report.total(),
        synced: report.synced.len(),
        not_found: report.not_found.len(),
        skipped: report.skipped.len(),
        failed: report.failed.len(),
        requeued: 0,
    }
}

/// 消费 Redis 队列：project_ids、version_ids、hashes 都归一成 project_id
pub async fn sync_queue(app: &App) -> Result<TaskSummary> {
    let mr = app.modrinth();
    let chunk = app.config.modrinth_chunk_size;
    let mut summary = TaskSummary::default();
    let mut targets: BTreeSet<String> = BTreeSet::new();

    let project_ids = app.queues.drain(key::MODRINTH_PROJECT_IDS).await?;
    targets.extend(project_ids.iter().cloned());

    let version_ids = app.queues.drain(key::MODRINTH_VERSION_IDS).await?;
    for batch in version_ids.chunks(chunk.max(1)) {
        match mr.api().get_versions(batch).await {
            Ok(versions) => targets.extend(versions.into_iter().map(|v| v.project_id)),
            Err(error) => {
                tracing::warn!(%error, count = batch.len(), "批量取版本失败");
                summary.requeued += requeue(&app.queues, key::MODRINTH_VERSION_IDS, batch).await?;
            }
        }
    }

    // Python 版这里遍历的是 sha1 与 sha256，而队列里实际是 sha1 与 sha512，
    // 于是 sha512 队列从来没被消费过，却每轮都被删掉
    for algorithm in key::MODRINTH_HASH_ALGORITHMS {
        let queue_key = key::modrinth_hashes(algorithm);
        let hashes = app.queues.drain(&queue_key).await?;
        tracing::info!(algorithm, count = hashes.len(), "取出 hash 队列");
        for batch in hashes.chunks(chunk.max(1)) {
            match mr.api().get_version_files(batch, algorithm).await {
                Ok(found) => targets.extend(found.into_values().map(|v| v.project_id)),
                Err(error) => {
                    tracing::warn!(%error, algorithm, count = batch.len(), "批量取 hash 失败");
                    summary.requeued += requeue(&app.queues, &queue_key, batch).await?;
                }
            }
        }
    }

    let targets: Vec<String> = targets.into_iter().collect();
    tracing::info!(count = targets.len(), "开始同步队列命中的项目");

    let report = mr.sync_projects(&targets).await;
    let requeued = summary.requeued;
    summary = summarize(&report);
    summary.requeued = requeued;

    let retry: Vec<String> = report.failed.iter().map(|(id, _)| id.clone()).collect();
    summary.requeued += requeue(&app.queues, key::MODRINTH_PROJECT_IDS, &retry).await?;

    Ok(summary)
}

/// 增量刷新：比对 updated / versions / game_versions，同时清理上游已删除的项目
pub async fn refresh(app: &App) -> Result<TaskSummary> {
    let mr = app.modrinth();

    // Diagnostic escape hatch: exercise the real project sync without waiting
    // for the full remote project scan to reach a known large project.
    if let Ok(project_id) = std::env::var("MCIM_ONLY_PROJECT_ID") {
        let project_id = project_id.trim().to_string();
        if !project_id.is_empty() {
            tracing::warn!(
                project_id,
                "diagnostic mode: syncing only one Modrinth project"
            );
            let report = mr.sync_projects(&[project_id]).await;
            return Ok(summarize(&report));
        }
    }

    let chunk = app.config.modrinth_chunk_size;

    let mut batches = app
        .db
        .chunked_all::<ProjectStamp>(
            collection::MODRINTH_PROJECTS,
            doc! { "_id": 1, "updated": 1, "versions": 1, "game_versions": 1 },
            chunk.max(1),
        )
        .await?;

    let mut total = 0usize;
    let mut outdated = Vec::new();
    let mut dead = Vec::new();
    let mut skipped = 0usize;
    while let Some(batch) = batches.next().await {
        let batch = batch?;
        total += batch.len();
        let ids: Vec<String> = batch.iter().map(|item| item.id.clone()).collect();
        let remote = match mr.api().get_projects(&ids).await {
            Ok(value) => value,
            Err(error) => {
                // 整批失败时不能把它们当成已删除
                tracing::warn!(%error, count = ids.len(), "批量取项目失败，本批跳过");
                skipped += 1;
                continue;
            }
        };
        // 这一批核对过了，不管内容有没有变。
        // 只是记账，写不进去也不该作废整轮
        if let Err(error) = app
            .db
            .touch_checked(collection::MODRINTH_PROJECTS, &ids, Utc::now())
            .await
        {
            tracing::warn!(%error, count = ids.len(), "checked_at 更新失败");
        }

        let alive: HashSet<&str> = remote.iter().map(|item| item.id.as_str()).collect();
        for item in &batch {
            if !alive.contains(item.id.as_str()) {
                dead.push(item.id.clone());
            }
        }
        let local_stamps: HashMap<&str, &ProjectStamp> =
            batch.iter().map(|item| (item.id.as_str(), item)).collect();
        for value in &remote {
            if let Some(item) = local_stamps.get(value.id.as_str())
                && is_outdated(item, value)
            {
                tracing::debug!(
                    id = value.id,
                    "项目有更新: local={local:?}, remote={remote:?}",
                    local = item,
                    remote = value
                );
                outdated.push(value.id.clone());
            }
        }
    }

    if skipped > 0 {
        tracing::warn!(batches = skipped, "有批次没比对上，本轮覆盖不完整");
    }
    let removed = remove_dead(&mr, &dead).await?;
    tracing::info!(total, count = outdated.len(), removed, "需要刷新的项目");

    let report = mr.sync_projects(&outdated).await;
    let summary = summarize(&report);
    Ok(summary)
}

/// 删除上游已消失的项目
async fn remove_dead(mr: &ModrinthSync, dead: &[String]) -> Result<usize> {
    if dead.is_empty() {
        return Ok(0);
    }

    let mut removed = 0usize;
    for project_id in dead {
        match mr.remove_project(project_id).await {
            Ok(counts) => {
                tracing::info!(
                    project_id,
                    projects = counts.projects,
                    versions = counts.versions,
                    files = counts.files,
                    translations = counts.translations,
                    "项目已删除"
                );
                removed += 1;
            }
            // 一个删不掉不该拖垮整轮刷新，下一轮还会认出它
            Err(error) => tracing::warn!(project_id, %error, "项目删除失败"),
        }
    }
    Ok(removed)
}

/// 一次取多少个项目 id 出来同步
///
/// 只是为了不把十几万个 id 和同样多的结果一直攥在手里，
/// 取得比并发数大得多，块尾等落单请求的损耗可以忽略
const REFRESH_FULL_CHUNK: usize = 1000;

pub async fn refresh_full(app: &App) -> Result<TaskSummary> {
    let mr = app.modrinth();
    let mut batches = app
        .db
        .chunked_all::<ProjectStamp>(
            collection::MODRINTH_PROJECTS,
            doc! { "_id": 1 },
            REFRESH_FULL_CHUNK,
        )
        .await?;
    tracing::info!("开始全量刷新");

    let mut summary = TaskSummary::default();
    while let Some(batch) = batches.next().await {
        let ids: Vec<String> = batch?.into_iter().map(|item| item.id).collect();
        let report = mr.sync_projects(&ids).await;
        let part = summarize(&report);
        summary.total += part.total;
        summary.synced += part.synced;
        summary.not_found += part.not_found;
        summary.skipped += part.skipped;
        summary.failed += part.failed;
        tracing::info!(done = summary.total, "全量刷新进度");
    }
    Ok(summary)
}

/// 按最新发布翻页，遇到已入库的项目就停
///
/// 每翻一页就同步这页的新项目，冷启动跑十几万条时进程中断也不会丢进度
pub async fn search(app: &App, max_pages: i64, full: bool) -> Result<TaskSummary> {
    let mr = app.modrinth();
    let mut summary = TaskSummary::default();
    let mut offset = 0i64;
    let mut pages = 0i64;

    loop {
        if max_pages > 0 && pages >= max_pages {
            tracing::info!(pages, "达到翻页上限");
            break;
        }
        let response = mr
            .api()
            .search_newest(offset, MODRINTH_SEARCH_PAGE_SIZE)
            .await?;
        if response.hits.is_empty() {
            break;
        }

        let page_ids: Vec<String> = response
            .hits
            .iter()
            .map(|hit| hit.project_id.clone())
            .collect();
        let existing = app
            .db
            .existing_ids(collection::MODRINTH_PROJECTS, &page_ids)
            .await?;
        let existing: HashSet<String> = existing
            .into_iter()
            .filter_map(|value| value.as_str().map(str::to_string))
            .collect();

        let fresh: Vec<String> = page_ids
            .iter()
            .filter(|id| !existing.contains(*id))
            .cloned()
            .collect();

        if !fresh.is_empty() {
            let report = mr.sync_projects(&fresh).await;
            summary.total += report.total();
            summary.synced += report.synced.len();
            summary.not_found += report.not_found.len();
            summary.skipped += report.skipped.len();
            summary.failed += report.failed.len();
            let retry: Vec<String> = report.failed.iter().map(|(id, _)| id.clone()).collect();
            summary.requeued += requeue(&app.queues, key::MODRINTH_PROJECT_IDS, &retry).await?;
        }

        tracing::info!(
            offset,
            total_hits = response.total_hits,
            fresh = fresh.len(),
            synced = summary.synced,
            "搜索翻页"
        );

        // 不满一页说明已经是最后一页
        if (response.hits.len() as i64) < MODRINTH_SEARCH_PAGE_SIZE {
            break;
        }
        // full 模式下不提前停，冷启动中断后重跑才能补上剩下的
        if !full && !existing.is_empty() {
            tracing::debug!(offset, "遇到已入库的项目，停止翻页");
            break;
        }
        offset += MODRINTH_SEARCH_PAGE_SIZE;
        pages += 1;
    }

    Ok(summary)
}

pub async fn tags(app: &App) -> Result<TaskSummary> {
    let mr = app.modrinth();
    let counts = mr.sync_tags().await?;
    let total = counts.categories + counts.loaders + counts.game_versions;
    tracing::info!(
        categories = counts.categories,
        loaders = counts.loaders,
        game_versions = counts.game_versions,
        "tags 同步完成"
    );
    Ok(TaskSummary {
        total,
        synced: total,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::{ProjectStamp, is_outdated, same_set};
    use crate::models::modrinth::Project;

    fn project(updated: &str, versions: &[&str], game_versions: &[&str]) -> Project {
        let value = serde_json::json!({
            "_id": "Wnxd13zP",
            "slug": "clumps",
            "team": "team",
            "published": "2020-01-01T00:00:00Z",
            "updated": updated,
            "followers": 1,
            "versions": versions,
            "game_versions": game_versions,
        });
        serde_json::from_value(value).expect("构造 Project 失败")
    }

    fn stamp(updated: &str, versions: &[&str], game_versions: &[&str]) -> ProjectStamp {
        let value = serde_json::json!({
            "_id": "Wnxd13zP",
            "updated": updated,
            "versions": versions,
            "game_versions": game_versions,
        });
        serde_json::from_value(value).expect("构造 ProjectStamp 失败")
    }

    #[test]
    fn identical_project_is_not_outdated() {
        let local = stamp("2026-06-09T23:48:35.117Z", &["a", "b"], &["1.20"]);
        let remote = project("2026-06-09T23:48:35.117961Z", &["a", "b"], &["1.20"]);
        assert!(!is_outdated(&local, &remote));
    }

    #[test]
    fn newer_updated_is_outdated() {
        let local = stamp("2026-06-09T23:48:35Z", &["a"], &["1.20"]);
        let remote = project("2026-06-09T23:49:00Z", &["a"], &["1.20"]);
        assert!(is_outdated(&local, &remote));
    }

    #[test]
    fn new_version_is_outdated() {
        let local = stamp("2026-06-09T23:48:35Z", &["a"], &["1.20"]);
        let remote = project("2026-06-09T23:48:35Z", &["a", "b"], &["1.20"]);
        assert!(is_outdated(&local, &remote));
    }

    /// 上游把版本列表重排不代表内容有变化
    #[test]
    fn reordered_versions_are_not_outdated() {
        let local = stamp("2026-06-09T23:48:35Z", &["a", "b"], &["1.20", "1.21"]);
        let remote = project("2026-06-09T23:48:35Z", &["b", "a"], &["1.21", "1.20"]);
        assert!(!is_outdated(&local, &remote));
    }

    #[test]
    fn set_comparison_ignores_order() {
        let left = vec!["a".to_string(), "b".to_string()];
        let right = vec!["b".to_string(), "a".to_string()];
        assert!(same_set(Some(&left), Some(&right)));
        assert!(same_set(None, None));
        assert!(!same_set(Some(&left), None));
    }
}
