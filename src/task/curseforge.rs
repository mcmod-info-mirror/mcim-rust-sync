use std::collections::{BTreeSet, HashMap, HashSet};

use bson::doc;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_with::serde_as;

use crate::api::curseforge::fingerprint_mod_ids;
use crate::app::App;
use crate::constants::{
    CURSEFORGE_SEARCH_LIMIT, CURSEFORGE_SEARCH_PAGE_SIZE, class_ids,
};
use crate::db::queue::key;
use crate::error::Result;
use crate::models::collection;
use crate::models::{FlexDateTime, curseforge::File};
use crate::sync::curseforge::CurseForgeSync;

use super::{TaskSummary, requeue, same_second};

/// 队列里混进过极小的 modid，实际不属于 Minecraft
const MIN_MOD_ID: i32 = 30000;

#[serde_as]
#[derive(Debug, Deserialize)]
struct ModStamp {
    #[serde(rename = "_id")]
    id: i32,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "dateModified", default)]
    date_modified: Option<DateTime<Utc>>,
}

fn parse_ids<T: std::str::FromStr>(raw: &[String]) -> (Vec<T>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut invalid = Vec::new();
    for value in raw {
        match value.parse::<T>() {
            Ok(id) => parsed.push(id),
            Err(_) => invalid.push(value.clone()),
        }
    }
    (parsed, invalid)
}

/// 消费 Redis 队列：modids、fileids、fingerprints 都归一成 modid 后统一同步
pub async fn sync_queue(app: &App) -> Result<TaskSummary> {
    let cf = app.curseforge()?;
    let chunk = app.config.curseforge_chunk_size;
    let mut summary = TaskSummary::default();

    let raw_mod_ids = app.queues.drain(key::CURSEFORGE_MODIDS).await?;
    let raw_file_ids = app.queues.drain(key::CURSEFORGE_FILEIDS).await?;
    let raw_fingerprints = app.queues.drain(key::CURSEFORGE_FINGERPRINTS).await?;
    tracing::info!(
        modids = raw_mod_ids.len(),
        fileids = raw_file_ids.len(),
        fingerprints = raw_fingerprints.len(),
        "队列已取出"
    );

    let (mod_ids, invalid) = parse_ids::<i32>(&raw_mod_ids);
    if !invalid.is_empty() {
        tracing::warn!(count = invalid.len(), "队列里有无法解析的 modid，已丢弃");
    }
    let mut targets: BTreeSet<i32> = mod_ids.into_iter().collect();

    let (file_ids, _) = parse_ids::<i32>(&raw_file_ids);
    summary.requeued += resolve_file_ids(&cf, app, &file_ids, chunk, &mut targets).await?;

    let (fingerprints, _) = parse_ids::<i64>(&raw_fingerprints);
    summary.requeued += resolve_fingerprints(&cf, app, &fingerprints, chunk, &mut targets).await?;

    let targets: Vec<i32> = targets.into_iter().filter(|id| *id >= MIN_MOD_ID).collect();
    tracing::info!(count = targets.len(), "开始同步队列命中的 mod");

    let report = cf.sync_mods(&targets).await;
    summary.total = report.total();
    summary.synced = report.synced.len();
    summary.not_found = report.not_found.len();
    summary.skipped = report.skipped.len();
    summary.failed = report.failed.len();

    // 只有真正失败的才放回，404 与不收录的直接丢弃，避免无限循环
    let retry: Vec<String> = report
        .failed
        .iter()
        .map(|(id, _)| id.to_string())
        .collect();
    summary.requeued += requeue(&app.queues, key::CURSEFORGE_MODIDS, &retry).await?;

    Ok(summary)
}

/// fileId 反查 modId，顺便把列表里不可见的文件强行入库
async fn resolve_file_ids(
    cf: &CurseForgeSync,
    app: &App,
    file_ids: &[i32],
    chunk_size: usize,
    targets: &mut BTreeSet<i32>,
) -> Result<usize> {
    let mut requeued = 0usize;
    for batch in file_ids.chunks(chunk_size.max(1)) {
        match cf.api().get_files(batch).await {
            Ok(files) => {
                targets.extend(files.iter().map(|file| file.mod_id));
                let hidden: Vec<File> = files
                    .into_iter()
                    .filter(|file| file.is_available == Some(false))
                    .collect();
                if !hidden.is_empty() {
                    tracing::debug!(count = hidden.len(), "文件在列表中不可见，直接入库");
                    cf.db()
                        .upsert_many(
                            collection::CURSEFORGE_FILES,
                            &hidden,
                            app.config.max_workers,
                        )
                        .await?;
                }
            }
            Err(error) => {
                // 这一批没处理成，放回队列而不是丢掉
                tracing::warn!(%error, count = batch.len(), "批量取文件失败");
                let back: Vec<String> = batch.iter().map(|id| id.to_string()).collect();
                requeued += requeue(&app.queues, key::CURSEFORGE_FILEIDS, &back).await?;
            }
        }
    }
    Ok(requeued)
}

async fn resolve_fingerprints(
    cf: &CurseForgeSync,
    app: &App,
    fingerprints: &[i64],
    chunk_size: usize,
    targets: &mut BTreeSet<i32>,
) -> Result<usize> {
    let mut requeued = 0usize;
    for batch in fingerprints.chunks(chunk_size.max(1)) {
        match cf.api().get_fingerprints(batch).await {
            Ok(result) => targets.extend(fingerprint_mod_ids(&result)),
            Err(error) => {
                tracing::warn!(%error, count = batch.len(), "批量取指纹失败");
                let back: Vec<String> = batch.iter().map(|id| id.to_string()).collect();
                requeued += requeue(&app.queues, key::CURSEFORGE_FINGERPRINTS, &back).await?;
            }
        }
    }
    Ok(requeued)
}

/// 增量刷新：比对上游 dateModified，只同步有变化的
pub async fn refresh(app: &App) -> Result<TaskSummary> {
    let cf = app.curseforge()?;
    let chunk = app.config.curseforge_chunk_size;

    let local: Vec<ModStamp> = app
        .db
        .stream_all(
            collection::CURSEFORGE_MODS,
            doc! { "_id": 1, "dateModified": 1 },
        )
        .await?;
    tracing::info!(count = local.len(), "库内 mod 总数");

    let mut outdated = Vec::new();
    for batch in local.chunks(chunk.max(1)) {
        let ids: Vec<i32> = batch
            .iter()
            .map(|item| item.id)
            .filter(|id| *id >= MIN_MOD_ID)
            .collect();
        if ids.is_empty() {
            continue;
        }
        let remote = cf.api().get_mods(&ids).await?;
        let local_stamps: HashMap<i32, Option<DateTime<Utc>>> = batch
            .iter()
            .map(|item| (item.id, item.date_modified))
            .collect();
        for value in remote {
            let local_stamp = local_stamps.get(&value.id).copied().flatten();
            if !same_second(local_stamp, value.date_modified) {
                outdated.push(value.id);
            }
        }
    }

    tracing::info!(count = outdated.len(), "需要刷新的 mod");
    let report = cf.sync_mods(&outdated).await;
    let mut summary = TaskSummary {
        total: report.total(),
        synced: report.synced.len(),
        not_found: report.not_found.len(),
        skipped: report.skipped.len(),
        failed: report.failed.len(),
        requeued: 0,
    };

    let retry: Vec<String> = report.failed.iter().map(|(id, _)| id.to_string()).collect();
    summary.requeued = requeue(&app.queues, key::CURSEFORGE_MODIDS, &retry).await?;
    Ok(summary)
}

/// 按发布时间倒序翻页，遇到已入库的就停
///
/// 每翻一页就把这页的新 mod 同步掉，进程中途挂掉也不会丢掉已发现的部分
pub async fn search(app: &App, game_id: i32, max_pages: i64) -> Result<TaskSummary> {
    let cf = app.curseforge()?;
    let mut summary = TaskSummary::default();

    for class_id in class_ids(game_id) {
        let mut index = 0i64;
        let mut pages = 0i64;
        while index + CURSEFORGE_SEARCH_PAGE_SIZE <= CURSEFORGE_SEARCH_LIMIT {
            if max_pages > 0 && pages >= max_pages {
                tracing::info!(game_id, class_id, pages, "达到翻页上限");
                break;
            }
            let response = cf
                .api()
                .search(game_id, Some(*class_id), index, CURSEFORGE_SEARCH_PAGE_SIZE)
                .await?;
            if response.pagination.result_count == 0 {
                break;
            }

            let page_ids: Vec<i32> = response.data.iter().map(|value| value.id).collect();
            let existing = app
                .db
                .existing_ids(collection::CURSEFORGE_MODS, &page_ids)
                .await?;
            let existing: HashSet<i32> = existing
                .into_iter()
                .filter_map(|value| value.as_i32().or_else(|| value.as_i64().map(|v| v as i32)))
                .collect();

            let fresh: Vec<i32> = page_ids
                .iter()
                .copied()
                .filter(|id| !existing.contains(id) && *id >= MIN_MOD_ID)
                .collect();

            if !fresh.is_empty() {
                let report = cf.sync_mods(&fresh).await;
                summary.total += report.total();
                summary.synced += report.synced.len();
                summary.not_found += report.not_found.len();
                summary.skipped += report.skipped.len();
                summary.failed += report.failed.len();
                let retry: Vec<String> =
                    report.failed.iter().map(|(id, _)| id.to_string()).collect();
                summary.requeued += requeue(&app.queues, key::CURSEFORGE_MODIDS, &retry).await?;
            }

            tracing::info!(
                game_id,
                class_id,
                index,
                fresh = fresh.len(),
                synced = summary.synced,
                "搜索翻页"
            );

            if !existing.is_empty() {
                tracing::debug!(class_id, index, "遇到已入库的 mod，停止翻页");
                break;
            }
            index += CURSEFORGE_SEARCH_PAGE_SIZE;
            pages += 1;
        }
    }

    Ok(summary)
}

pub async fn categories(app: &App, game_id: i32) -> Result<TaskSummary> {
    let cf = app.curseforge()?;
    let categories = cf.sync_categories(game_id).await?;
    tracing::info!(game_id, count = categories.len(), "分类同步完成");
    Ok(TaskSummary {
        total: categories.len(),
        synced: categories.len(),
        ..Default::default()
    })
}
