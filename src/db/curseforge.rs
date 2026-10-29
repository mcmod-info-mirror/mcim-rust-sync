//! CurseForge 表的读写。表结构见 mcim-rust-api 的 migrations。

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use super::postgres::{Database, allocate_ids, flatten_string_lists};
use crate::error::Result;
use crate::models::curseforge::{
    Author, Category, CategoryInfo, File, FileDependencies, FileIndex, FileInfo,
    FileSortableGameVersions, Hash, Links, Logo, Mod, Module, ScreenShot,
};
use crate::models::translate::CurseForgeTranslation;

const FILE_RECORD_SEQUENCE: &str = "SELECT nextval(pg_get_serial_sequence('curseforge_file_records', 'record_id')) \
     FROM generate_series(1, $1)";

/// 删除一个 mod 时各表实际删掉的条数
#[derive(Debug, Clone, Copy, Default)]
pub struct RemovedMod {
    pub mods: u64,
    pub files: u64,
    pub translations: u64,
}

/// `curseforge_file_records` 一行的内容；`File` 与 `Mod.latestFiles` 共用这张表
struct FileRecord<'a> {
    id: i32,
    game_id: i32,
    mod_id: i32,
    is_available: Option<bool>,
    display_name: Option<&'a str>,
    file_name: Option<&'a str>,
    release_type: Option<i32>,
    file_status: Option<i32>,
    hashes: &'a [Hash],
    file_date: Option<DateTime<Utc>>,
    file_length: Option<i64>,
    download_count: Option<i64>,
    file_size_on_disk: Option<i64>,
    download_url: Option<&'a str>,
    game_versions: Option<&'a Vec<String>>,
    sortable_game_versions: &'a [FileSortableGameVersions],
    dependencies: &'a [FileDependencies],
    expose_as_alternative: Option<bool>,
    parent_project_file_id: Option<i32>,
    alternate_file_id: Option<i32>,
    is_server_pack: Option<bool>,
    server_pack_file_id: Option<i32>,
    is_early_access_content: Option<bool>,
    early_access_end_date: Option<DateTime<Utc>>,
    file_fingerprint: Option<i64>,
    modules: &'a [Module],
}

macro_rules! file_record_from {
    ($ty:ty) => {
        impl<'a> From<&'a $ty> for FileRecord<'a> {
            fn from(f: &'a $ty) -> Self {
                Self {
                    id: f.id,
                    game_id: f.game_id,
                    mod_id: f.mod_id,
                    is_available: f.is_available,
                    display_name: f.display_name.as_deref(),
                    file_name: f.file_name.as_deref(),
                    release_type: f.release_type,
                    file_status: f.file_status,
                    hashes: f.hashes.as_deref().unwrap_or_default(),
                    file_date: f.file_date,
                    file_length: f.file_length,
                    download_count: f.download_count,
                    file_size_on_disk: f.file_size_on_disk,
                    download_url: f.download_url.as_deref(),
                    game_versions: f.game_versions.as_ref(),
                    sortable_game_versions: f.sortable_game_versions.as_deref().unwrap_or_default(),
                    dependencies: f.dependencies.as_deref().unwrap_or_default(),
                    expose_as_alternative: f.expose_as_alternative,
                    parent_project_file_id: f.parent_project_file_id,
                    alternate_file_id: f.alternate_file_id,
                    is_server_pack: f.is_server_pack,
                    server_pack_file_id: f.server_pack_file_id,
                    is_early_access_content: f.is_early_access_content,
                    early_access_end_date: f.early_access_end_date,
                    file_fingerprint: f.file_fingerprint,
                    modules: f.modules.as_deref().unwrap_or_default(),
                }
            }
        }
    };
}

file_record_from!(File);
file_record_from!(FileInfo);

/// 写入文件记录及其子表；record_id 须由 FILE_RECORD_SEQUENCE 预分配
async fn insert_file_records(
    conn: &mut PgConnection,
    records: &[(i64, FileRecord<'_>)],
) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO curseforge_file_records (record_id, id, game_id, mod_id, is_available, \
         display_name, file_name, release_type, file_status, file_date, file_length, \
         download_count, file_size_on_disk, download_url, expose_as_alternative, \
         parent_project_file_id, alternate_file_id, is_server_pack, server_pack_file_id, \
         is_early_access_content, early_access_end_date, file_fingerprint) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::integer[], $4::integer[], \
             $5::boolean[], $6::text[], $7::text[], $8::integer[], $9::integer[], \
             $10::timestamptz[], $11::bigint[], $12::bigint[], $13::bigint[], $14::text[], \
             $15::boolean[], $16::integer[], $17::integer[], $18::boolean[], $19::integer[], \
             $20::boolean[], $21::timestamptz[], $22::bigint[])",
    )
    .bind(records.iter().map(|(id, _)| *id).collect::<Vec<_>>())
    .bind(records.iter().map(|(_, f)| f.id).collect::<Vec<_>>())
    .bind(records.iter().map(|(_, f)| f.game_id).collect::<Vec<_>>())
    .bind(records.iter().map(|(_, f)| f.mod_id).collect::<Vec<_>>())
    .bind(
        records
            .iter()
            .map(|(_, f)| f.is_available)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.display_name)
            .collect::<Vec<_>>(),
    )
    .bind(records.iter().map(|(_, f)| f.file_name).collect::<Vec<_>>())
    .bind(
        records
            .iter()
            .map(|(_, f)| f.release_type)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.file_status)
            .collect::<Vec<_>>(),
    )
    .bind(records.iter().map(|(_, f)| f.file_date).collect::<Vec<_>>())
    .bind(
        records
            .iter()
            .map(|(_, f)| f.file_length)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.download_count)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.file_size_on_disk)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.download_url)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.expose_as_alternative)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.parent_project_file_id)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.alternate_file_id)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.is_server_pack)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.server_pack_file_id)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.is_early_access_content)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.early_access_end_date)
            .collect::<Vec<_>>(),
    )
    .bind(
        records
            .iter()
            .map(|(_, f)| f.file_fingerprint)
            .collect::<Vec<_>>(),
    )
    .execute(&mut *conn)
    .await?;

    let hashes: Vec<(i64, i32, &Hash)> = records
        .iter()
        .flat_map(|(id, f)| {
            f.hashes
                .iter()
                .enumerate()
                .map(move |(i, h)| (*id, i as i32, h))
        })
        .collect();
    sqlx::query(
        "INSERT INTO curseforge_file_record_hashes (record_id, position, value, algo) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::text[], $4::integer[])",
    )
    .bind(hashes.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())
    .bind(hashes.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
    .bind(
        hashes
            .iter()
            .map(|(_, _, h)| h.value.as_str())
            .collect::<Vec<_>>(),
    )
    .bind(hashes.iter().map(|(_, _, h)| h.algo).collect::<Vec<_>>())
    .execute(&mut *conn)
    .await?;

    let (parents, positions, values) =
        flatten_string_lists(records.iter().map(|(id, f)| (*id, f.game_versions)));
    sqlx::query(
        "INSERT INTO curseforge_file_record_game_versions (record_id, position, game_version) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::text[])",
    )
    .bind(parents)
    .bind(positions)
    .bind(values)
    .execute(&mut *conn)
    .await?;

    let sortable: Vec<(i64, i32, &FileSortableGameVersions)> = records
        .iter()
        .flat_map(|(id, f)| {
            f.sortable_game_versions
                .iter()
                .enumerate()
                .map(move |(i, v)| (*id, i as i32, v))
        })
        .collect();
    sqlx::query(
        "INSERT INTO curseforge_file_record_sortable_game_versions (record_id, position, \
         game_version_name, game_version_padded, game_version, game_version_release_date, \
         game_version_type_id) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::text[], $4::text[], $5::text[], \
             $6::timestamptz[], $7::integer[])",
    )
    .bind(sortable.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())
    .bind(sortable.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
    .bind(
        sortable
            .iter()
            .map(|(_, _, v)| v.game_version_name.as_deref())
            .collect::<Vec<_>>(),
    )
    .bind(
        sortable
            .iter()
            .map(|(_, _, v)| v.game_version_padded.as_deref())
            .collect::<Vec<_>>(),
    )
    .bind(
        sortable
            .iter()
            .map(|(_, _, v)| v.game_version.as_deref())
            .collect::<Vec<_>>(),
    )
    .bind(
        sortable
            .iter()
            .map(|(_, _, v)| v.game_version_release_date)
            .collect::<Vec<_>>(),
    )
    .bind(
        sortable
            .iter()
            .map(|(_, _, v)| v.game_version_type_id)
            .collect::<Vec<_>>(),
    )
    .execute(&mut *conn)
    .await?;

    let dependencies: Vec<(i64, i32, &FileDependencies)> = records
        .iter()
        .flat_map(|(id, f)| {
            f.dependencies
                .iter()
                .enumerate()
                .map(move |(i, d)| (*id, i as i32, d))
        })
        .collect();
    sqlx::query(
        "INSERT INTO curseforge_file_record_dependencies (record_id, position, mod_id, relation_type) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::integer[], $4::integer[])",
    )
    .bind(dependencies.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())
    .bind(dependencies.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
    .bind(dependencies.iter().map(|(_, _, d)| d.mod_id).collect::<Vec<_>>())
    .bind(dependencies.iter().map(|(_, _, d)| d.relation_type).collect::<Vec<_>>())
    .execute(&mut *conn)
    .await?;

    let modules: Vec<(i64, i32, &Module)> = records
        .iter()
        .flat_map(|(id, f)| {
            f.modules
                .iter()
                .enumerate()
                .map(move |(i, m)| (*id, i as i32, m))
        })
        .collect();
    sqlx::query(
        "INSERT INTO curseforge_file_record_modules (record_id, position, name, fingerprint) \
         SELECT * FROM UNNEST($1::bigint[], $2::integer[], $3::text[], $4::bigint[])",
    )
    .bind(modules.iter().map(|(id, _, _)| *id).collect::<Vec<_>>())
    .bind(modules.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
    .bind(
        modules
            .iter()
            .map(|(_, _, m)| m.name.as_deref())
            .collect::<Vec<_>>(),
    )
    .bind(
        modules
            .iter()
            .map(|(_, _, m)| m.fingerprint)
            .collect::<Vec<_>>(),
    )
    .execute(&mut *conn)
    .await?;

    Ok(())
}

/// 同一批里重复的 id 只保留最后一次出现的，否则批量插入会撞主键
fn last_by_id<T>(items: &[T], id: impl Fn(&T) -> i32) -> Vec<&T> {
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<&T> = items
        .iter()
        .rev()
        .filter(|item| seen.insert(id(item)))
        .collect();
    kept.reverse();
    kept
}

impl Database {
    /// 按文件 id 整份替换 `curseforge_files`
    ///
    /// 沿用整文档替换语义，上游已删除的字段会一并消失
    pub async fn replace_curseforge_files(&self, files: &[File]) -> Result<u64> {
        let files = last_by_id(files, |f| f.id);
        if files.is_empty() {
            return Ok(0);
        }
        let ids: Vec<i32> = files.iter().map(|f| f.id).collect();
        let mut tx = self
            .begin_locked("curseforge_file", ids.iter().map(i32::to_string).collect())
            .await?;

        // 删掉文件记录即级联删掉 curseforge_files 行与全部子表行
        sqlx::query(
            "DELETE FROM curseforge_file_records WHERE record_id IN \
             (SELECT record_id FROM curseforge_files WHERE id = ANY($1))",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;

        let record_ids = allocate_ids(&mut tx, FILE_RECORD_SEQUENCE, files.len()).await?;
        let records: Vec<(i64, FileRecord)> = record_ids
            .iter()
            .copied()
            .zip(files.iter().map(|f| FileRecord::from(*f)))
            .collect();
        insert_file_records(&mut tx, &records).await?;

        sqlx::query(
            "INSERT INTO curseforge_files (id, record_id, sync_at) \
             SELECT * FROM UNNEST($1::integer[], $2::bigint[], $3::timestamptz[])",
        )
        .bind(&ids)
        .bind(&record_ids)
        .bind(files.iter().map(|f| f.sync_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(files.len() as u64)
    }

    /// 删掉某个 mod 下 isAvailable 为真、却已不在 `kept` 里的文件
    ///
    /// 有些文件在列表里不可见但按 fileId 仍能取到，那些要留着
    pub async fn delete_unlisted_curseforge_files(&self, mod_id: i32, kept: &[i32]) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM curseforge_file_records WHERE record_id IN ( \
                 SELECT f.record_id FROM curseforge_files f \
                 JOIN curseforge_file_records r ON r.record_id = f.record_id \
                 WHERE r.mod_id = $1 AND r.is_available AND f.id <> ALL($2))",
        )
        .bind(mod_id)
        .bind(kept)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// 按 mod id 整份替换 `curseforge_mods`（含 latestFiles 等全部嵌套结构）
    pub async fn replace_curseforge_mods(&self, mods: &[Mod]) -> Result<u64> {
        let mods = last_by_id(mods, |m| m.id);
        if mods.is_empty() {
            return Ok(0);
        }
        let ids: Vec<i32> = mods.iter().map(|m| m.id).collect();
        let mut tx = self
            .begin_locked("curseforge_mod", ids.iter().map(i32::to_string).collect())
            .await?;

        sqlx::query(
            "DELETE FROM curseforge_file_records WHERE record_id IN \
             (SELECT record_id FROM curseforge_mod_latest_files WHERE mod_id = ANY($1))",
        )
        .bind(&ids)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM curseforge_mods WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            "INSERT INTO curseforge_mods (id, game_id, name, slug, summary, status, download_count, \
             is_featured, primary_category_id, class_id, main_file_id, date_created, date_modified, \
             date_released, allow_mod_distribution, game_popularity_rank, is_available, \
             thumbs_up_count, rating, sync_at, checked_at) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::text[], $4::text[], $5::text[], \
                 $6::integer[], $7::bigint[], $8::boolean[], $9::integer[], $10::integer[], \
                 $11::integer[], $12::timestamptz[], $13::timestamptz[], $14::timestamptz[], \
                 $15::boolean[], $16::integer[], $17::boolean[], $18::integer[], $19::integer[], \
                 $20::timestamptz[], $21::timestamptz[])",
        )
        .bind(&ids)
        .bind(mods.iter().map(|m| m.game_id).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.name.as_deref()).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.slug.as_str()).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.summary.as_deref()).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.status).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.download_count).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.is_featured).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.primary_category_id).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.class_id).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.main_file_id).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.date_created).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.date_modified).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.date_released).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.allow_mod_distribution).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.game_popularity_rank).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.is_available).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.thumbs_up_count).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.rating).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.sync_at).collect::<Vec<_>>())
        .bind(mods.iter().map(|m| m.checked_at).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let links: Vec<(i32, &Links)> = mods
            .iter()
            .filter_map(|m| Some((m.id, m.links.as_ref()?)))
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_links (mod_id, website_url, wiki_url, issues_url, source_url) \
             SELECT * FROM UNNEST($1::integer[], $2::text[], $3::text[], $4::text[], $5::text[])",
        )
        .bind(links.iter().map(|(id, _)| *id).collect::<Vec<_>>())
        .bind(links.iter().map(|(_, l)| l.website_url.as_deref()).collect::<Vec<_>>())
        .bind(links.iter().map(|(_, l)| l.wiki_url.as_deref()).collect::<Vec<_>>())
        .bind(links.iter().map(|(_, l)| l.issues_url.as_deref()).collect::<Vec<_>>())
        .bind(links.iter().map(|(_, l)| l.source_url.as_deref()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let logos: Vec<(i32, &Logo)> = mods
            .iter()
            .filter_map(|m| Some((m.id, m.logo.as_ref()?)))
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_logos (mod_id, id, logo_mod_id, title, description, \
             thumbnail_url, url) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::integer[], $4::text[], \
                 $5::text[], $6::text[], $7::text[])",
        )
        .bind(logos.iter().map(|(id, _)| *id).collect::<Vec<_>>())
        .bind(logos.iter().map(|(_, l)| l.id).collect::<Vec<_>>())
        .bind(logos.iter().map(|(_, l)| l.mod_id).collect::<Vec<_>>())
        .bind(
            logos
                .iter()
                .map(|(_, l)| l.title.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            logos
                .iter()
                .map(|(_, l)| l.description.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            logos
                .iter()
                .map(|(_, l)| l.thumbnail_url.as_deref())
                .collect::<Vec<_>>(),
        )
        .bind(
            logos
                .iter()
                .map(|(_, l)| l.url.as_deref())
                .collect::<Vec<_>>(),
        )
        .execute(&mut *tx)
        .await?;

        let categories: Vec<(i32, i32, &CategoryInfo)> = mods
            .iter()
            .flat_map(|m| {
                m.categories
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, c)| (m.id, i as i32, c))
            })
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_categories (mod_id, position, id, game_id, name, slug, url, \
             icon_url, date_modified, is_class, class_id, parent_category_id, display_index) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::integer[], $4::integer[], \
                 $5::text[], $6::text[], $7::text[], $8::text[], $9::timestamptz[], $10::boolean[], \
                 $11::integer[], $12::integer[], $13::integer[])",
        )
        .bind(categories.iter().map(|(m, _, _)| *m).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.id).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.game_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.name.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.slug.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.url.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.icon_url.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.date_modified).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.is_class).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.class_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.parent_category_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|(_, _, c)| c.display_index).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let authors: Vec<(i32, i32, &Author)> = mods
            .iter()
            .flat_map(|m| {
                m.authors
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, a)| (m.id, i as i32, a))
            })
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_authors (mod_id, position, id, name, url) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::integer[], $4::text[], $5::text[])",
        )
        .bind(authors.iter().map(|(m, _, _)| *m).collect::<Vec<_>>())
        .bind(authors.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
        .bind(authors.iter().map(|(_, _, a)| a.id).collect::<Vec<_>>())
        .bind(authors.iter().map(|(_, _, a)| a.name.as_str()).collect::<Vec<_>>())
        .bind(authors.iter().map(|(_, _, a)| a.url.as_deref()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let screenshots: Vec<(i32, i32, &ScreenShot)> = mods
            .iter()
            .flat_map(|m| {
                m.screenshots
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, s)| (m.id, i as i32, s))
            })
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_screenshots (mod_id, position, id, screenshot_mod_id, title, \
             description, thumbnail_url, url) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::integer[], $4::integer[], \
                 $5::text[], $6::text[], $7::text[], $8::text[])",
        )
        .bind(screenshots.iter().map(|(m, _, _)| *m).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.id).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.mod_id).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.title.as_deref()).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.description.as_deref()).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.thumbnail_url.as_deref()).collect::<Vec<_>>())
        .bind(screenshots.iter().map(|(_, _, s)| s.url.as_deref()).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let indexes: Vec<(i32, i32, &FileIndex)> = mods
            .iter()
            .flat_map(|m| {
                m.latest_files_indexes
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, x)| (m.id, i as i32, x))
            })
            .collect();
        sqlx::query(
            "INSERT INTO curseforge_mod_latest_files_indexes (mod_id, position, game_version, file_id, \
             filename, release_type, game_version_type_id, mod_loader) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::text[], $4::integer[], \
                 $5::text[], $6::integer[], $7::integer[], $8::integer[])",
        )
        .bind(indexes.iter().map(|(m, _, _)| *m).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.game_version.as_deref()).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.file_id).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.filename.as_deref()).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.release_type).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.game_version_type_id).collect::<Vec<_>>())
        .bind(indexes.iter().map(|(_, _, x)| x.mod_loader).collect::<Vec<_>>())
        .execute(&mut *tx)
        .await?;

        let latest: Vec<(i32, i32, &FileInfo)> = mods
            .iter()
            .flat_map(|m| {
                m.latest_files
                    .iter()
                    .flatten()
                    .enumerate()
                    .map(move |(i, f)| (m.id, i as i32, f))
            })
            .collect();
        let record_ids = allocate_ids(&mut tx, FILE_RECORD_SEQUENCE, latest.len()).await?;
        let records: Vec<(i64, FileRecord)> = record_ids
            .iter()
            .copied()
            .zip(latest.iter().map(|(_, _, f)| FileRecord::from(*f)))
            .collect();
        insert_file_records(&mut tx, &records).await?;
        sqlx::query(
            "INSERT INTO curseforge_mod_latest_files (mod_id, position, record_id) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::bigint[])",
        )
        .bind(latest.iter().map(|(m, _, _)| *m).collect::<Vec<_>>())
        .bind(latest.iter().map(|(_, p, _)| *p).collect::<Vec<_>>())
        .bind(&record_ids)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(mods.len() as u64)
    }

    pub async fn upsert_curseforge_categories(&self, categories: &[Category]) -> Result<u64> {
        let categories = last_by_id(categories, |c| c.id);
        if categories.is_empty() {
            return Ok(0);
        }
        let result = sqlx::query(
            "INSERT INTO curseforge_categories (id, game_id, name, slug, url, icon_url, date_modified, \
             is_class, class_id, parent_category_id, display_index, sync_at) \
             SELECT * FROM UNNEST($1::integer[], $2::integer[], $3::text[], $4::text[], $5::text[], \
                 $6::text[], $7::timestamptz[], $8::boolean[], $9::integer[], $10::integer[], \
                 $11::integer[], $12::timestamptz[]) \
             ON CONFLICT (id) DO UPDATE SET game_id = EXCLUDED.game_id, name = EXCLUDED.name, \
                 slug = EXCLUDED.slug, url = EXCLUDED.url, icon_url = EXCLUDED.icon_url, \
                 date_modified = EXCLUDED.date_modified, is_class = EXCLUDED.is_class, \
                 class_id = EXCLUDED.class_id, parent_category_id = EXCLUDED.parent_category_id, \
                 display_index = EXCLUDED.display_index, sync_at = EXCLUDED.sync_at",
        )
        .bind(categories.iter().map(|c| c.id).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.game_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.name.as_str()).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.slug.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.url.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.icon_url.as_deref()).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.date_modified).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.is_class).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.class_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.parent_category_id).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.display_index).collect::<Vec<_>>())
        .bind(categories.iter().map(|c| c.sync_at).collect::<Vec<_>>())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn curseforge_translation(
        &self,
        mod_id: i32,
    ) -> Result<Option<CurseForgeTranslation>> {
        Ok(sqlx::query_as(
            "SELECT mod_id, translated, original, need_to_update, translated_at \
             FROM curseforge_translated WHERE mod_id = $1",
        )
        .bind(mod_id)
        .fetch_optional(&self.pool)
        .await?)
    }

    pub async fn upsert_curseforge_translation(
        &self,
        record: &CurseForgeTranslation,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO curseforge_translated (mod_id, translated, original, need_to_update, \
             translated_at) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (mod_id) DO UPDATE SET translated = EXCLUDED.translated, \
                 original = EXCLUDED.original, need_to_update = EXCLUDED.need_to_update, \
                 translated_at = EXCLUDED.translated_at",
        )
        .bind(record.mod_id)
        .bind(&record.translated)
        .bind(&record.original)
        .bind(record.need_to_update)
        .bind(record.translated_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 删掉一个 mod，连同它的文件与翻译记录
    pub async fn remove_curseforge_mod(&self, mod_id: i32) -> Result<RemovedMod> {
        let mut tx = self
            .begin_locked("curseforge_mod", vec![mod_id.to_string()])
            .await?;
        let files = sqlx::query(
            "DELETE FROM curseforge_file_records WHERE record_id IN ( \
                 SELECT f.record_id FROM curseforge_files f \
                 JOIN curseforge_file_records r ON r.record_id = f.record_id \
                 WHERE r.mod_id = $1)",
        )
        .bind(mod_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let translations = sqlx::query("DELETE FROM curseforge_translated WHERE mod_id = $1")
            .bind(mod_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        sqlx::query(
            "DELETE FROM curseforge_file_records WHERE record_id IN \
             (SELECT record_id FROM curseforge_mod_latest_files WHERE mod_id = $1)",
        )
        .bind(mod_id)
        .execute(&mut *tx)
        .await?;
        let mods = sqlx::query("DELETE FROM curseforge_mods WHERE id = $1")
            .bind(mod_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(RemovedMod {
            mods,
            files,
            translations,
        })
    }

    /// 这批 mod id 里已经入库的部分
    pub async fn existing_curseforge_mod_ids(&self, ids: &[i32]) -> Result<Vec<i32>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM curseforge_mods WHERE id = ANY($1)")
                .bind(ids)
                .fetch_all(&self.pool)
                .await?,
        )
    }

    /// 标记这批 mod 在此刻被核对过
    pub async fn touch_curseforge_mods_checked(
        &self,
        ids: &[i32],
        at: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query("UPDATE curseforge_mods SET checked_at = $2 WHERE id = ANY($1)")
            .bind(ids)
            .bind(at)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// 按 id 升序分页取 (id, dateModified)，`after` 为上一页最后一个 id
    ///
    /// 用键集分页而不是一直开着游标，长时间的比对过程中不占着连接
    pub async fn curseforge_mod_stamps(
        &self,
        after: Option<i32>,
        limit: usize,
    ) -> Result<Vec<(i32, Option<DateTime<Utc>>)>> {
        Ok(sqlx::query_as(
            "SELECT id, date_modified FROM curseforge_mods \
             WHERE $1::integer IS NULL OR id > $1 ORDER BY id LIMIT $2",
        )
        .bind(after)
        .bind(limit.max(1) as i64)
        .fetch_all(&self.pool)
        .await?)
    }
}
