use bson::doc;
use chrono::Utc;
use futures::stream::{self, StreamExt};

use crate::api::ModrinthApi;
use crate::db::Database;
use crate::error::{Error, Result};
use crate::models::collection;
use crate::models::modrinth::{File, Project, Version};
use crate::models::translate::ModrinthTranslation;

use super::{Outcome, Report};

#[derive(Debug, Clone)]
pub struct ProjectSummary {
    pub id: String,
    pub slug: String,
    pub version_count: usize,
}

#[derive(Debug, Clone, Default)]
pub struct TagCounts {
    pub categories: usize,
    pub loaders: usize,
    pub game_versions: usize,
}

pub struct ModrinthSync {
    api: ModrinthApi,
    db: Database,
    concurrency: usize,
}

impl ModrinthSync {
    pub fn new(api: ModrinthApi, db: Database, concurrency: usize) -> Self {
        Self {
            api,
            db,
            concurrency: concurrency.max(1),
        }
    }

    pub fn db(&self) -> &Database {
        &self.db
    }

    pub fn api(&self) -> &ModrinthApi {
        &self.api
    }

    /// 同步单个项目：先写版本与文件，再写翻译标记，最后才写 Project 文档
    pub async fn sync_project(&self, project_id: &str) -> Result<Outcome<ProjectSummary>> {
        let project = match self.api.get_project(project_id).await {
            Ok(value) => value,
            Err(e) if e.is_not_found() => return Ok(Outcome::NotFound),
            Err(e) => return Err(e),
        };

        let version_count = self.sync_project_versions(project_id).await?;

        self.sync_translation(project_id, project.description.as_deref())
            .await?;

        let slug = project.slug.clone();
        self.db
            .upsert_many(collection::MODRINTH_PROJECTS, &[project], 1)
            .await?;

        Ok(Outcome::Synced(ProjectSummary {
            id: project_id.to_string(),
            slug,
            version_count,
        }))
    }

    pub async fn sync_projects(&self, project_ids: &[String]) -> Report<String, ProjectSummary> {
        let mut report = Report::default();
        let mut results = stream::iter(project_ids.iter().cloned())
            .map(|project_id| async move {
                let outcome = self.sync_project(&project_id).await;
                (project_id, outcome)
            })
            .buffer_unordered(self.concurrency);

        while let Some((project_id, result)) = results.next().await {
            if let Err(error) = &result {
                tracing::warn!(project_id, %error, "项目同步失败");
            }
            report.record(project_id, result);
        }
        report
    }

    /// 拉全部版本，写入版本与文件，再清掉已经消失的
    async fn sync_project_versions(&self, project_id: &str) -> Result<usize> {
        let versions = self.api.get_project_versions(project_id).await?;
        if versions.is_empty() {
            return Err(Error::Config(format!(
                "项目 {} 的版本列表为空，响应可能不完整",
                project_id
            )));
        }

        let stamp = Utc::now();
        let mut files = Vec::new();
        for version in &versions {
            for info in &version.files {
                files.push(File {
                    hashes: info.hashes.clone(),
                    url: info.url.clone(),
                    filename: info.filename.clone(),
                    primary: info.primary,
                    size: info.size,
                    file_type: info.file_type.clone(),
                    version_id: version.id.clone(),
                    project_id: version.project_id.clone(),
                    sync_at: stamp,
                });
            }
        }

        self.db
            .upsert_many(collection::MODRINTH_FILES, &files, self.concurrency)
            .await?;
        self.db
            .upsert_many(collection::MODRINTH_VERSIONS, &versions, self.concurrency)
            .await?;

        let kept: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        let removed_versions = self
            .db
            .delete_many(
                collection::MODRINTH_VERSIONS,
                doc! { "project_id": project_id, "_id": { "$nin": &kept } },
            )
            .await?;
        let removed_files = self
            .db
            .delete_many(
                collection::MODRINTH_FILES,
                doc! { "project_id": project_id, "version_id": { "$nin": &kept } },
            )
            .await?;

        tracing::info!(
            project_id,
            versions = versions.len(),
            files = files.len(),
            removed_versions,
            removed_files,
            "项目版本同步完成"
        );
        Ok(versions.len())
    }

    async fn sync_translation(&self, project_id: &str, description: Option<&str>) -> Result<()> {
        let existing = self
            .db
            .collection::<ModrinthTranslation>(collection::MODRINTH_TRANSLATED)
            .find_one(doc! { "_id": project_id })
            .await?;

        let unchanged = existing
            .as_ref()
            .is_some_and(|record| record.original.as_deref() == description);
        if unchanged {
            return Ok(());
        }

        let record = ModrinthTranslation {
            project_id: project_id.to_string(),
            translated: existing.as_ref().and_then(|r| r.translated.clone()),
            original: description.map(str::to_string),
            need_to_update: true,
            translated_at: existing.as_ref().and_then(|r| r.translated_at),
        };
        self.db
            .upsert_many(collection::MODRINTH_TRANSLATED, &[record], 1)
            .await?;
        Ok(())
    }

    /// 刷新 categories / loaders / game_versions
    ///
    /// 三份数据全部取回来才动库，任何一个失败都不会留下半张表
    pub async fn sync_tags(&self) -> Result<TagCounts> {
        let categories = self.api.get_categories().await?;
        let loaders = self.api.get_loaders().await?;
        let game_versions = self.api.get_game_versions().await?;

        let stamp = Utc::now();
        let categories = stamp_all(categories, |item| &mut item.sync_at, stamp);
        let loaders = stamp_all(loaders, |item| &mut item.sync_at, stamp);
        let game_versions = stamp_all(game_versions, |item| &mut item.sync_at, stamp);

        self.db
            .refresh_collection(collection::MODRINTH_CATEGORIES, &categories, stamp)
            .await?;
        self.db
            .refresh_collection(collection::MODRINTH_LOADERS, &loaders, stamp)
            .await?;
        self.db
            .refresh_collection(collection::MODRINTH_GAME_VERSIONS, &game_versions, stamp)
            .await?;

        Ok(TagCounts {
            categories: categories.len(),
            loaders: loaders.len(),
            game_versions: game_versions.len(),
        })
    }

    /// 删除上游已经消失的项目，连同它的版本与文件
    ///
    /// Python 版这里删的是 `modrinth_hashes` 集合，而真实集合叫 `modrinth_files`，
    /// 所以文件文档从来没被删掉过，一直在库里变成孤儿数据
    pub async fn remove_project(&self, project_id: &str) -> Result<(u64, u64, u64)> {
        let files = self
            .db
            .delete_many(
                collection::MODRINTH_FILES,
                doc! { "project_id": project_id },
            )
            .await?;
        let versions = self
            .db
            .delete_many(
                collection::MODRINTH_VERSIONS,
                doc! { "project_id": project_id },
            )
            .await?;
        let projects = self
            .db
            .delete_by_id(collection::MODRINTH_PROJECTS, &project_id)
            .await?;
        Ok((projects, versions, files))
    }
}

fn stamp_all<T>(
    mut items: Vec<T>,
    field: impl Fn(&mut T) -> &mut chrono::DateTime<Utc>,
    stamp: chrono::DateTime<Utc>,
) -> Vec<T> {
    for item in items.iter_mut() {
        *field(item) = stamp;
    }
    items
}

/// 批量取项目概要，用于判断哪些需要同步、哪些已经不存在
pub async fn fetch_projects(
    api: &ModrinthApi,
    project_ids: &[String],
    chunk_size: usize,
) -> Result<Vec<Project>> {
    let mut all = Vec::new();
    for chunk in project_ids.chunks(chunk_size.max(1)) {
        all.extend(api.get_projects(chunk).await?);
    }
    Ok(all)
}

/// 批量取版本，用于把 version_id 反查成 project_id
pub async fn fetch_versions(
    api: &ModrinthApi,
    version_ids: &[String],
    chunk_size: usize,
) -> Result<Vec<Version>> {
    let mut all = Vec::new();
    for chunk in version_ids.chunks(chunk_size.max(1)) {
        all.extend(api.get_versions(chunk).await?);
    }
    Ok(all)
}
