//! Modrinth 表的读写。表结构见 mcim-rust-api 的 migrations。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use super::postgres::{Database, allocate_ids, flatten_string_lists};
use crate::error::{Error, Result};
use crate::models::modrinth::{
    Category, Dependencies, DonationUrl, File, FileInfo, GalleryItem, GameVersion, License, Loader,
    Project, Version,
};
use crate::models::translate::ModrinthTranslation;

const LOADER_SEQUENCE: &str = "SELECT nextval(pg_get_serial_sequence('modrinth_loaders', 'id')) \
                               FROM generate_series(1, $1)";

/// 删除一个项目时各表实际删掉的条数
#[derive(Debug, Clone, Copy, Default)]
pub struct RemovedProject {
    pub projects: u64,
    pub versions: u64,
    pub files: u64,
    pub translations: u64,
}

/// 增量刷新比对用的本地快照
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProjectStamp {
    pub id: String,
    pub updated: Option<DateTime<Utc>>,
    pub versions: Vec<String>,
    pub game_versions: Vec<String>,
}

/// 同一批里重复的 id 只保留最后一次出现的，否则批量插入会撞主键
fn last_by_id<T>(items: &[T], id: impl Fn(&T) -> &str) -> Vec<&T> {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<&T> = items
        .iter()
        .rev()
        .filter(|item| seen.insert(id(item).to_string()))
        .collect();
    kept.reverse();
    kept
}

/// 写入一组 `parent -> [String]` 子表；`sql` 为 (text[], integer[], text[]) 的 UNNEST 插入
async fn insert_string_lists<'a>(
    conn: &mut PgConnection,
    sql: &'static str,
    lists: impl IntoIterator<Item = (&'a str, Option<&'a Vec<String>>)>,
) -> Result<()> {
    let (parents, positions, values) = flatten_string_lists(lists);
    sqlx::query(sql)
        .bind(parents)
        .bind(positions)
        .bind(values)
        .execute(conn)
        .await?;
    Ok(())
}

impl Database {
    /// 按项目 id 整份替换 `modrinth_projects`
    pub async fn replace_modrinth_projects(&self, projects: &[Project]) -> Result<u64> {
        let projects = last_by_id(projects, |p| &p.id);
        if projects.is_empty() {
            return Ok(0);
        }
        let ids: Vec<&str> = projects.iter().map(|p| p.id.as_str()).collect();
        let mut tx = self
            .begin_locked(
                "modrinth_project",
                ids.iter().map(|id| id.to_string()).collect(),
            )
            .await?;

        sqlx::query("DELETE FROM modrinth_projects WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO modrinth_projects (id, slug, title, description, client_side, server_side, \
             body, status, requested_status, issues_url, source_url, wiki_url, discord_url, \
             project_type, downloads, icon_url, color, thread_id, monetization_status, team, \
             body_url, published, updated, approved, queued, followers, sync_at, checked_at) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::text[], $8::text[], $9::text[], $10::text[], $11::text[], \
                 $12::text[], $13::text[], $14::text[], $15::bigint[], $16::text[], $17::bigint[], \
                 $18::text[], $19::text[], $20::text[], $21::text[], $22::timestamptz[], \
                 $23::timestamptz[], $24::timestamptz[], $25::timestamptz[], $26::bigint[], \
                 $27::timestamptz[], $28::timestamptz[])",
        )
        .bind(&ids)
        .bind(projects.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.title.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.description.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.client_side.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.server_side.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.body.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.status.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.requested_status.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.issues_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.source_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.wiki_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.discord_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.project_type.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.downloads).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.icon_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.color.map(i64::from)).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.thread_id.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.monetization_status.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.team.as_str()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.body_url.as_deref()).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.published).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.updated).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.approved).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.queued).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| i64::from(p.followers)).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.sync_at).collect::<Vec<_>>())
        .bind(projects.iter().map(|p| p.checked_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_project_categories (project_id, position, category) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            projects
                .iter()
                .map(|p| (p.id.as_str(), p.categories.as_ref())),
        )
        .await?;
        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_project_additional_categories (project_id, position, category) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            projects
                .iter()
                .map(|p| (p.id.as_str(), p.additional_categories.as_ref())),
        )
        .await?;
        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_project_versions (project_id, position, version_id) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            projects
                .iter()
                .map(|p| (p.id.as_str(), p.versions.as_ref())),
        )
        .await?;
        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_project_game_versions (project_id, position, game_version) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            projects
                .iter()
                .map(|p| (p.id.as_str(), p.game_versions.as_ref())),
        )
        .await?;
        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_project_loaders (project_id, position, loader) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            projects.iter().map(|p| (p.id.as_str(), p.loaders.as_ref())),
        )
        .await?;

        let donations: Vec<(&str, i32, &DonationUrl)> = projects
            .iter()
            .flat_map(|p| {
                p.donation_urls
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, d)| (p.id.as_str(), i as i32, d))
            })
            .collect();
        sqlx::query(
            "INSERT INTO modrinth_project_donation_urls (project_id, position, id, platform, url) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[], $4::text[], $5::text[])",
        )
        .bind(donations.iter().map(|(p, _, _)| *p).collect::<Vec<_>>())
        .bind(donations.iter().map(|(_, i, _)| *i).collect::<Vec<_>>())
        .bind(
            donations
                .iter()
                .map(|(_, _, d)| d.id.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            donations
                .iter()
                .map(|(_, _, d)| d.platform.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            donations
                .iter()
                .map(|(_, _, d)| d.url.as_deref())
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;

        let gallery: Vec<(&str, i32, &GalleryItem)> = projects
            .iter()
            .flat_map(|p| {
                p.gallery
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, g)| (p.id.as_str(), i as i32, g))
            })
            .collect();
        sqlx::query(
            "INSERT INTO modrinth_project_gallery (project_id, position, url, featured, title, \
             description, created, ordering) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[], $4::boolean[], $5::text[], \
                 $6::text[], $7::timestamptz[], $8::bigint[])",
        )
        .bind(gallery.iter().map(|(p, _, _)| *p).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, i, _)| *i).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.url.as_str()).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.featured).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.title.as_deref()).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.description.as_deref()).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.created).collect::<Vec<_>>())
        .bind(gallery.iter().map(|(_, _, g)| g.ordering).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let licenses: Vec<(&str, &License)> = projects
            .iter()
            .filter_map(|p| Some((p.id.as_str(), p.license.as_ref()?)))
            .collect();
        sqlx::query(
            "INSERT INTO modrinth_project_licenses (project_id, id, name, url) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[])",
        )
        .bind(licenses.iter().map(|(p, _)| *p).collect::<Vec<_>>())
        .bind(
            licenses
                .iter()
                .map(|(_, l)| l.id.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            licenses
                .iter()
                .map(|(_, l)| l.name.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            licenses
                .iter()
                .map(|(_, l)| l.url.as_deref())
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(projects.len() as u64)
    }

    /// 按版本 id 整份替换 `modrinth_versions`
    pub async fn replace_modrinth_versions(&self, versions: &[Version]) -> Result<u64> {
        let versions = last_by_id(versions, |v| &v.id);
        if versions.is_empty() {
            return Ok(0);
        }
        let ids: Vec<&str> = versions.iter().map(|v| v.id.as_str()).collect();
        let mut tx = self
            .begin_locked(
                "modrinth_version",
                ids.iter().map(|id| id.to_string()).collect(),
            )
            .await?;

        sqlx::query("DELETE FROM modrinth_versions WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO modrinth_versions (id, project_id, name, version_number, changelog, \
             version_type, featured, status, requested_status, author_id, date_published, downloads, \
             changelog_url, sync_at) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::boolean[], $8::text[], $9::text[], $10::text[], \
                 $11::timestamptz[], $12::bigint[], $13::text[], $14::timestamptz[])",
        )
        .bind(&ids)
        .bind(versions.iter().map(|v| v.project_id.as_str()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.name.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.version_number.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.changelog.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.version_type.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.featured).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.status.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.requested_status.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.author_id.as_str()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.date_published).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.downloads).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.changelog_url.as_deref()).collect::<Vec<_>>())
        .bind(versions.iter().map(|v| v.sync_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let deps: Vec<(&str, i32, &Dependencies)> = versions
            .iter()
            .flat_map(|v| {
                v.dependencies
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, d)| (v.id.as_str(), i as i32, d))
            })
            .collect();
        sqlx::query(
            "INSERT INTO modrinth_version_dependencies (version_id, position, dependency_version_id, \
             dependency_project_id, file_name, dependency_type) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[], $4::text[], $5::text[], \
                 $6::text[])",
        )
        .bind(deps.iter().map(|(v, _, _)| *v).collect::<Vec<_>>())
        .bind(deps.iter().map(|(_, i, _)| *i).collect::<Vec<_>>())
        .bind(deps.iter().map(|(_, _, d)| d.version_id.as_deref()).collect::<Vec<_>>())
        .bind(deps.iter().map(|(_, _, d)| d.project_id.as_deref()).collect::<Vec<_>>())
        .bind(deps.iter().map(|(_, _, d)| d.file_name.as_deref()).collect::<Vec<_>>())
        .bind(deps.iter().map(|(_, _, d)| d.dependency_type.as_str()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_version_game_versions (version_id, position, game_version) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            versions
                .iter()
                .map(|v| (v.id.as_str(), v.game_versions.as_ref())),
        )
        .await?;
        insert_string_lists(
            &mut tx,
            "INSERT INTO modrinth_version_loaders (version_id, position, loader) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[])",
            versions.iter().map(|v| (v.id.as_str(), v.loaders.as_ref())),
        )
        .await?;

        let files: Vec<(&str, i32, &FileInfo)> = versions
            .iter()
            .flat_map(|v| {
                v.files
                    .iter()
                    .enumerate()
                    .map(move |(i, f)| (v.id.as_str(), i as i32, f))
            })
            .collect();
        sqlx::query(
            "INSERT INTO modrinth_version_files (version_id, position, sha1, sha512, url, filename, \
             \"primary\", size, file_type) \
             SELECT * FROM UNNEST($1::text[], $2::integer[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::boolean[], $8::bigint[], $9::text[])",
        )
        .bind(files.iter().map(|(v, _, _)| *v).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, i, _)| *i).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.hashes.sha1.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.hashes.sha512.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.url.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.filename.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.primary).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.size).collect::<Vec<_>>())
        .bind(files.iter().map(|(_, _, f)| f.file_type.as_deref()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(versions.len() as u64)
    }

    /// 插入或整行覆盖 `modrinth_files`，主键为 (sha1, sha512)
    pub async fn upsert_modrinth_files(&self, files: &[File]) -> Result<u64> {
        // 按主键排序、去重：同一批里重复的键会让 ON CONFLICT 报错，
        // 固定顺序则让并发写入同一批文件的事务不会互相死锁
        let mut files: Vec<&File> = files.iter().collect();
        files.sort_by(|a, b| {
            (&a.hashes.sha1, &a.hashes.sha512).cmp(&(&b.hashes.sha1, &b.hashes.sha512))
        });
        files.dedup_by(|a, b| a.hashes == b.hashes);
        if files.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query(
            "INSERT INTO modrinth_files (sha1, sha512, url, filename, \"primary\", size, file_type, \
             version_id, project_id, sync_at) \
             SELECT * FROM UNNEST($1::text[], $2::text[], $3::text[], $4::text[], $5::boolean[], \
                 $6::bigint[], $7::text[], $8::text[], $9::text[], $10::timestamptz[]) \
             ON CONFLICT (sha1, sha512) DO UPDATE SET url = EXCLUDED.url, \
                 filename = EXCLUDED.filename, \"primary\" = EXCLUDED.\"primary\", \
                 size = EXCLUDED.size, file_type = EXCLUDED.file_type, \
                 version_id = EXCLUDED.version_id, project_id = EXCLUDED.project_id, \
                 file_cdn_cached = NULL, found = NULL, sync_at = EXCLUDED.sync_at",
        )
        .bind(files.iter().map(|f| f.hashes.sha1.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.hashes.sha512.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.url.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.filename.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.primary).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.size).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.file_type.as_deref()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.version_id.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.project_id.as_str()).collect::<Vec<_>>())
        .bind(files.iter().map(|f| f.sync_at).collect::<Vec<_>>())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// 删掉某个项目下不在 `kept` 里的版本
    pub async fn delete_stale_modrinth_versions(
        &self,
        project_id: &str,
        kept: &[&str],
    ) -> Result<u64> {
        let result =
            sqlx::query("DELETE FROM modrinth_versions WHERE project_id = $1 AND id <> ALL($2)")
                .bind(project_id)
                .bind(kept)
                .execute(&self.pool)
                .await?;
        Ok(result.rows_affected())
    }

    /// 删掉某个项目下所属版本已不在 `kept_versions` 里的文件
    pub async fn delete_stale_modrinth_files(
        &self,
        project_id: &str,
        kept_versions: &[&str],
    ) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM modrinth_files WHERE project_id = $1 AND version_id <> ALL($2)",
        )
        .bind(project_id)
        .bind(kept_versions)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn modrinth_translation(
        &self,
        project_id: &str,
    ) -> Result<Option<ModrinthTranslation>> {
        Ok(sqlx::query_as(
            "SELECT project_id, translated, original, need_to_update, translated_at \
             FROM modrinth_translated WHERE project_id = $1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn upsert_modrinth_translation(&self, record: &ModrinthTranslation) -> Result<()> {
        sqlx::query(
            "INSERT INTO modrinth_translated (project_id, translated, original, need_to_update, \
             translated_at) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (project_id) DO UPDATE SET translated = EXCLUDED.translated, \
                 original = EXCLUDED.original, need_to_update = EXCLUDED.need_to_update, \
                 translated_at = EXCLUDED.translated_at",
        )
        .bind(&record.project_id)
        .bind(&record.translated)
        .bind(&record.original)
        .bind(record.need_to_update)
        .bind(record.translated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 在一个事务里整表替换 categories / loaders / game_versions
    ///
    /// 读方在提交前一直看到上一轮的完整数据，任何一步失败都会整体回滚
    pub async fn refresh_modrinth_tags(
        &self,
        categories: &[Category],
        loaders: &[Loader],
        game_versions: &[GameVersion],
    ) -> Result<()> {
        for (name, empty) in [
            ("modrinth_categories", categories.is_empty()),
            ("modrinth_loaders", loaders.is_empty()),
            ("modrinth_game_versions", game_versions.is_empty()),
        ] {
            if empty {
                return Err(Error::Config(format!("{} 的新数据为空，拒绝刷新", name)));
            }
        }

        let mut tx = self.pool.begin().await?;

        sqlx::query("DELETE FROM modrinth_categories")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO modrinth_categories (icon, name, project_type, header, sync_at) \
             SELECT icon, name, project_type, header, sync_at FROM UNNEST($1::text[], $2::text[], \
                 $3::text[], $4::text[], $5::timestamptz[]) \
             WITH ORDINALITY AS c(icon, name, project_type, header, sync_at, ord) ORDER BY ord",
        )
        .bind(
            categories
                .iter()
                .map(|c| c.icon.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(
            categories
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(
            categories
                .iter()
                .map(|c| c.project_type.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            categories
                .iter()
                .map(|c| c.header.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(categories.iter().map(|c| c.sync_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM modrinth_loaders")
            .execute(&mut *tx)
            .await?;
        let mut loader_ids = allocate_ids(&mut tx, LOADER_SEQUENCE, loaders.len()).await?;
        // 序列值递增，排序后与传入顺序一一对应，接口按 id 排序即可还原顺序
        loader_ids.sort_unstable();
        sqlx::query(
            "INSERT INTO modrinth_loaders (id, icon, name, sync_at) \
             SELECT * FROM UNNEST($1::bigint[], $2::text[], $3::text[], $4::timestamptz[])",
        )
        .bind(&loader_ids)
        .bind(loaders.iter().map(|l| l.icon.as_str()).collect::<Vec<_>>())
        .bind(loaders.iter().map(|l| l.name.as_str()).collect::<Vec<_>>())
        .bind(loaders.iter().map(|l| l.sync_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;
        let (parents, positions, values) = flatten_string_lists(
            loader_ids
                .iter()
                .copied()
                .zip(loaders.iter().map(|l| Some(&l.supported_project_types))),
        );
        sqlx::query(
            "INSERT INTO modrinth_loader_supported_project_types (loader_id, position, project_type) \
             SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::text[])",
        )
        .bind(parents)
        .bind(positions)
        .bind(values)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM modrinth_game_versions")
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO modrinth_game_versions (version, version_type, date, major, sync_at) \
             SELECT version, version_type, date, major, sync_at FROM UNNEST($1::text[], \
                 $2::text[], $3::timestamptz[], $4::boolean[], $5::timestamptz[]) \
             WITH ORDINALITY AS g(version, version_type, date, major, sync_at, ord) ORDER BY ord",
        )
        .bind(
            game_versions
                .iter()
                .map(|g| g.version.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(
            game_versions
                .iter()
                .map(|g| g.version_type.as_str())
                .collect::<Vec<_>>(),
        )
        .bind(game_versions.iter().map(|g| g.date).collect::<Vec<_>>())
        .bind(game_versions.iter().map(|g| g.major).collect::<Vec<_>>())
        .bind(game_versions.iter().map(|g| g.sync_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// 删掉一个项目，连同它的版本、文件与翻译记录
    pub async fn remove_modrinth_project(&self, project_id: &str) -> Result<RemovedProject> {
        let mut tx = self
            .begin_locked("modrinth_project", vec![project_id.to_string()])
            .await?;
        let files = sqlx::query("DELETE FROM modrinth_files WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let versions = sqlx::query("DELETE FROM modrinth_versions WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let translations = sqlx::query("DELETE FROM modrinth_translated WHERE project_id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        let projects = sqlx::query("DELETE FROM modrinth_projects WHERE id = $1")
            .bind(project_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(RemovedProject {
            projects,
            versions,
            files,
            translations,
        })
    }

    /// 这批项目 id 里已经入库的部分
    pub async fn existing_modrinth_project_ids(&self, ids: &[String]) -> Result<Vec<String>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM modrinth_projects WHERE id = ANY($1)")
                .bind(ids)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// 标记这批项目在此刻被核对过
    pub async fn touch_modrinth_projects_checked(
        &self,
        ids: &[String],
        at: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query("UPDATE modrinth_projects SET checked_at = $2 WHERE id = ANY($1)")
            .bind(ids)
            .bind(at)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// 按 id 升序分页取比对用的快照，`after` 为上一页最后一个 id
    pub async fn modrinth_project_stamps(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<ProjectStamp>> {
        Ok(sqlx::query_as(
            "SELECT p.id, p.updated, \
                 coalesce((SELECT array_agg(v.version_id ORDER BY v.position) \
                     FROM modrinth_project_versions v WHERE v.project_id = p.id), '{}') AS versions, \
                 coalesce((SELECT array_agg(g.game_version ORDER BY g.position) \
                     FROM modrinth_project_game_versions g WHERE g.project_id = p.id), '{}') \
                     AS game_versions \
             FROM modrinth_projects p WHERE $1::text IS NULL OR p.id > $1 ORDER BY p.id LIMIT $2",
        )
        .bind(after)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?)
    }

    /// 按 id 升序分页取项目 id，`after` 为上一页最后一个 id
    pub async fn modrinth_project_ids(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM modrinth_projects WHERE $1::text IS NULL OR id > $1 ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?)
    }
}
