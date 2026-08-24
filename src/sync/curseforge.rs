use bson::doc;
use futures::stream::{self, StreamExt};

use crate::api::CurseForgeApi;
use crate::constants::{
    ACCEPT_GAME_IDS, CURSEFORGE_FILES_FALLBACK_PAGE_SIZE, CURSEFORGE_FILES_PAGE_SIZE,
};
use crate::db::Database;
use crate::error::Result;
use crate::models::collection;
use crate::models::curseforge::{Category, File, Mod};
use crate::models::translate::CurseForgeTranslation;

use super::{Outcome, Report};

#[derive(Debug, Clone)]
pub struct ModSummary {
    pub id: i32,
    pub name: Option<String>,
    pub file_count: usize,
}

pub struct CurseForgeSync {
    api: CurseForgeApi,
    db: Database,
    concurrency: usize,
}

impl CurseForgeSync {
    pub fn new(api: CurseForgeApi, db: Database, concurrency: usize) -> Self {
        Self {
            api,
            db,
            concurrency: concurrency.max(1),
        }
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn api(&self) -> &CurseForgeApi {
        &self.api
    }

    /// 同步单个 mod：先写文件，再写翻译标记，最后才写 Mod 文档
    ///
    /// Mod 放最后是为了避免文件没刷成功却更新了 Mod 的 dateModified，
    /// 那样下一轮增量刷新会认为它已是最新而跳过
    pub async fn sync_mod(&self, mod_id: i32) -> Result<Outcome<ModSummary>> {
        let mod_model = match self.api.get_mod(mod_id).await {
            Ok(value) => value,
            Err(e) if e.is_not_found() => return Ok(Outcome::NotFound),
            Err(e) => return Err(e),
        };

        if !mod_model
            .game_id
            .is_some_and(|game_id| ACCEPT_GAME_IDS.contains(&game_id))
        {
            tracing::debug!(mod_id, game_id = ?mod_model.game_id, "gameId 不在收录范围，跳过");
            return Ok(Outcome::Skipped);
        }

        let files = self.fetch_all_files(mod_id).await?;
        let file_count = files.len();
        self.db
            .upsert_many(collection::CURSEFORGE_FILES, &files, self.concurrency)
            .await?;

        let kept: Vec<i32> = files.iter().map(|file| file.id).collect();
        // isAvailable 为真却已从文件列表消失的才删。
        // 有些文件在列表里不可见但按 fileId 仍能取到，那些要留着
        let removed = self
            .db
            .delete_many(
                collection::CURSEFORGE_FILES,
                doc! {
                    "modId": mod_id,
                    "isAvailable": true,
                    "_id": { "$nin": kept },
                },
            )
            .await?;

        self.sync_translation(mod_id, mod_model.summary.as_deref())
            .await?;

        let name = mod_model.name.clone();
        self.db
            .upsert_many(collection::CURSEFORGE_MODS, &[mod_model], 1)
            .await?;

        tracing::info!(mod_id, file_count, removed, "mod 同步完成");
        Ok(Outcome::Synced(ModSummary {
            id: mod_id,
            name,
            file_count,
        }))
    }

    /// 并发同步一批 mod，失败的 id 会被带回来
    pub async fn sync_mods(&self, mod_ids: &[i32]) -> Report<i32, ModSummary> {
        let mut report = Report::default();
        let mut results = stream::iter(mod_ids.iter().copied())
            .map(|mod_id| async move { (mod_id, self.sync_mod(mod_id).await) })
            .buffer_unordered(self.concurrency);

        while let Some((mod_id, result)) = results.next().await {
            if let Err(error) = &result {
                tracing::warn!(mod_id, %error, "mod 同步失败");
            }
            report.record(mod_id, result);
        }
        report
    }

    /// 先尝试一次拉完，响应不完整再退回逐页
    ///
    /// Python 版在不完整时把 pageSize 减 1 重试三次，实际拿不到剩余数据
    async fn fetch_all_files(&self, mod_id: i32) -> Result<Vec<File>> {
        let first = self
            .api
            .get_mod_files(mod_id, 0, CURSEFORGE_FILES_PAGE_SIZE)
            .await?;
        let page = &first.pagination;
        if page.result_count == page.total_count && first.data.len() as i64 == page.result_count {
            return Ok(first.data);
        }

        tracing::warn!(
            mod_id,
            result_count = page.result_count,
            total_count = page.total_count,
            returned = first.data.len(),
            "一次性拉取的文件列表不完整，改为逐页拉取"
        );

        let mut files = Vec::new();
        let mut index = 0i64;
        loop {
            let response = self
                .api
                .get_mod_files(mod_id, index, CURSEFORGE_FILES_FALLBACK_PAGE_SIZE)
                .await?;
            let total = response.pagination.total_count;
            let returned = response.data.len() as i64;
            files.extend(response.data);
            index += returned.max(1);
            if returned == 0 || index >= total {
                break;
            }
        }
        Ok(files)
    }

    /// 按 fileId 直接补录，用于文件在列表中不可见但实际存在的情况
    pub async fn sync_files_by_ids(&self, file_ids: &[i32], chunk_size: usize) -> Result<usize> {
        let mut written = 0usize;
        for chunk in file_ids.chunks(chunk_size.max(1)) {
            let files = self.api.get_files(chunk).await?;
            written += files.len();
            self.db
                .upsert_many(collection::CURSEFORGE_FILES, &files, self.concurrency)
                .await?;
        }
        Ok(written)
    }

    /// 只做标记，真正的翻译由 mcim-translate 完成
    async fn sync_translation(&self, mod_id: i32, summary: Option<&str>) -> Result<()> {
        let existing = self
            .db
            .collection::<CurseForgeTranslation>(collection::CURSEFORGE_TRANSLATED)
            .find_one(doc! { "_id": mod_id })
            .await?;

        let unchanged = existing
            .as_ref()
            .is_some_and(|record| record.original.as_deref() == summary);
        if unchanged {
            return Ok(());
        }

        let record = CurseForgeTranslation {
            mod_id,
            translated: existing.as_ref().and_then(|r| r.translated.clone()),
            original: summary.map(str::to_string),
            need_to_update: true,
            translated_at: existing.as_ref().and_then(|r| r.translated_at),
        };
        self.db
            .upsert_many(collection::CURSEFORGE_TRANSLATED, &[record], 1)
            .await?;
        Ok(())
    }

    pub async fn sync_categories(&self, game_id: i32) -> Result<Vec<Category>> {
        let categories = self.api.get_categories(game_id, None, false).await?;
        self.db
            .upsert_many(
                collection::CURSEFORGE_CATEGORIES,
                &categories,
                self.concurrency,
            )
            .await?;
        Ok(categories)
    }

    /// 上游已删除的 mod，连同它的文件一起清掉
    ///
    /// Python 版在 CurseForge 侧完全没有这条路径，404 的 mod 会永远留在库里
    pub async fn remove_mod(&self, mod_id: i32) -> Result<(u64, u64)> {
        let files = self
            .db
            .delete_many(collection::CURSEFORGE_FILES, doc! { "modId": mod_id })
            .await?;
        let mods = self
            .db
            .delete_by_id(collection::CURSEFORGE_MODS, &mod_id)
            .await?;
        Ok((mods, files))
    }
}

/// 取一批 mod 的概要，用于判断是否需要同步
pub async fn fetch_mods(api: &CurseForgeApi, mod_ids: &[i32], chunk_size: usize) -> Result<Vec<Mod>> {
    let mut all = Vec::new();
    for chunk in mod_ids.chunks(chunk_size.max(1)) {
        all.extend(api.get_mods(chunk).await?);
    }
    Ok(all)
}
