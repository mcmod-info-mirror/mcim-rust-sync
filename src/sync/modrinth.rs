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

/// 删除一个项目时各集合实际删掉的条数
#[derive(Debug, Clone, Copy, Default)]
pub struct RemovedCounts {
    pub projects: u64,
    pub versions: u64,
    pub files: u64,
    pub translations: u64,
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
            // Each project response can contain a large version/file graph.
            // Keep the memory-heavy part bounded even if the environment sets
            // MAX_WORKERS higher.
            concurrency: concurrency.clamp(1, 2),
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
        let mut project = match self.api.get_project(project_id).await {
            Ok(value) => value,
            Err(e) if e.is_not_found() => return Ok(Outcome::NotFound),
            Err(e) => return Err(e),
        };

        // 项目自己声明的版本数，用来判断空响应是真的没版本还是响应不完整
        let declared = project.versions.as_ref().map_or(0, Vec::len);
        let version_count = match self.sync_project_versions(project_id, declared).await {
            Ok(count) => count,
            // 取完项目再取版本，这中间项目可能已经被删了，这种不该重试
            Err(e) if e.is_not_found() => {
                tracing::info!(project_id, "项目在两次请求之间消失了");
                return Ok(Outcome::NotFound);
            }
            Err(e) => return Err(e),
        };

        self.sync_translation(project_id, project.description.as_deref())
            .await?;

        let slug = project.slug.clone();
        // 刚从上游取回来，等同于刚核对过
        project.checked_at = Some(project.sync_at);
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
    ///
    /// 版本列表为空有两种可能：项目确实一个版本都没发过，或者上游这次响应不完整。
    /// 后者不能当成真值处理，否则裁剪会把该项目已有的版本和文件全删掉。
    /// 用项目自己声明的 `versions` 长度来区分
    async fn sync_project_versions(&self, project_id: &str, declared: usize) -> Result<usize> {
        let profile_project = crate::profile::project_enabled(project_id);
        if profile_project {
            crate::profile::dump("before-versions-request", Some(project_id));
        }
        let versions = self.api.get_project_versions(project_id).await?;
        if profile_project {
            crate::profile::dump("after-versions-response", Some(project_id));
        }
        if is_incomplete_versions(versions.len(), declared) {
            return Err(Error::Config(format!(
                "项目 {} 声明有 {} 个版本，版本接口却返回空，响应不完整",
                project_id, declared
            )));
        }
        if versions.is_empty() {
            tracing::debug!(project_id, "项目还没有发布任何版本");
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
        let file_count = files.len();
        if profile_project {
            tracing::info!(
                project_id,
                versions = versions.len(),
                files = file_count,
                "profile data graph built"
            );
            crate::profile::dump("after-files-built", Some(project_id));
        }

        self.db
            .upsert_many(collection::MODRINTH_FILES, &files, self.concurrency)
            .await?;
        if profile_project {
            crate::profile::dump("after-files-upsert", Some(project_id));
        }
        drop(files);
        self.db
            .upsert_many(collection::MODRINTH_VERSIONS, &versions, self.concurrency)
            .await?;
        if profile_project {
            crate::profile::dump("after-versions-upsert", Some(project_id));
        }

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
            files = file_count,
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

    /// 删除上游已经消失的项目，连同它的版本、文件与翻译记录
    ///
    /// Python 版这里删的是 `modrinth_hashes` 集合，而真实集合叫 `modrinth_files`，
    /// 所以文件文档从来没被删掉过；翻译记录同样没删，会一直堆积
    pub async fn remove_project(&self, project_id: &str) -> Result<RemovedCounts> {
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
        let translations = self
            .db
            .delete_by_id(collection::MODRINTH_TRANSLATED, &project_id)
            .await?;
        let projects = self
            .db
            .delete_by_id(collection::MODRINTH_PROJECTS, &project_id)
            .await?;
        Ok(RemovedCounts {
            projects,
            versions,
            files,
            translations,
        })
    }
}

/// 版本接口返回空时，判断是项目本来就没有版本还是响应不完整
///
/// 不加区分地把空当真值会让裁剪逻辑删光该项目已有的版本与文件；
/// 反过来一律当失败，则新建但还没发版的项目永远存不进库、还会一直重试
fn is_incomplete_versions(returned: usize, declared: usize) -> bool {
    returned == 0 && declared > 0
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

#[cfg(test)]
mod tests {
    use super::is_incomplete_versions;

    #[test]
    fn brand_new_project_without_versions_is_fine() {
        // 上游确实一个版本都没有，项目本身也这么声明
        assert!(!is_incomplete_versions(0, 0));
    }

    #[test]
    fn empty_response_for_a_project_with_versions_is_incomplete() {
        // 声明有版本却返回空，八成是上游抽风，不能据此删数据
        assert!(is_incomplete_versions(0, 12));
    }

    #[test]
    fn non_empty_response_is_always_accepted() {
        assert!(!is_incomplete_versions(12, 12));
        // 数量对不上不算响应不完整，可能只是两次调用之间发了新版本
        assert!(!is_incomplete_versions(11, 12));
        assert!(!is_incomplete_versions(13, 12));
    }
}
