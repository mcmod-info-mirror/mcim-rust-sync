use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::{FlexDateTime, now};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hash {
    pub value: String,
    pub algo: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Author {
    pub id: i32,
    pub name: String,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Logo {
    pub id: i32,
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "thumbnailUrl", default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenShot {
    pub id: i32,
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "thumbnailUrl", default)]
    pub thumbnail_url: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Links {
    #[serde(rename = "websiteUrl", default)]
    pub website_url: Option<String>,
    #[serde(rename = "wikiUrl", default)]
    pub wiki_url: Option<String>,
    #[serde(rename = "issuesUrl", default)]
    pub issues_url: Option<String>,
    #[serde(rename = "sourceUrl", default)]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub fingerprint: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDependencies {
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(rename = "relationType", default)]
    pub relation_type: Option<i32>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSortableGameVersions {
    #[serde(rename = "gameVersionName", default)]
    pub game_version_name: Option<String>,
    #[serde(rename = "gameVersionPadded", default)]
    pub game_version_padded: Option<String>,
    #[serde(rename = "gameVersion", default)]
    pub game_version: Option<String>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "gameVersionReleaseDate", default)]
    pub game_version_release_date: Option<DateTime<Utc>>,
    #[serde(rename = "gameVersionTypeId", default)]
    pub game_version_type_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileIndex {
    #[serde(rename = "gameVersion", default)]
    pub game_version: Option<String>,
    #[serde(rename = "fileId")]
    pub file_id: i32,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(rename = "releaseType", default)]
    pub release_type: Option<i32>,
    #[serde(rename = "gameVersionTypeId", default)]
    pub game_version_type_id: Option<i32>,
    #[serde(rename = "modLoader", default)]
    pub mod_loader: Option<i32>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategoryInfo {
    #[serde(default)]
    pub id: Option<i32>,
    #[serde(rename = "gameId", default)]
    pub game_id: Option<i32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "iconUrl", default)]
    pub icon_url: Option<String>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "dateModified", default)]
    pub date_modified: Option<DateTime<Utc>>,
    #[serde(rename = "isClass", default)]
    pub is_class: Option<bool>,
    #[serde(rename = "classId", default)]
    pub class_id: Option<i32>,
    #[serde(rename = "parentCategoryId", default)]
    pub parent_category_id: Option<i32>,
    #[serde(rename = "displayIndex", default)]
    pub display_index: Option<i32>,
}

/// `curseforge_files` 集合
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    #[serde(rename = "_id", alias = "id")]
    pub id: i32,
    #[serde(rename = "gameId")]
    pub game_id: i32,
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(rename = "isAvailable", default)]
    pub is_available: Option<bool>,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(rename = "fileName", default)]
    pub file_name: Option<String>,
    #[serde(rename = "releaseType", default)]
    pub release_type: Option<i32>,
    #[serde(rename = "fileStatus", default)]
    pub file_status: Option<i32>,
    #[serde(default)]
    pub hashes: Option<Vec<Hash>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "fileDate", default)]
    pub file_date: Option<DateTime<Utc>>,
    #[serde(rename = "fileLength", default)]
    pub file_length: Option<i64>,
    #[serde(rename = "downloadCount", default)]
    pub download_count: Option<i64>,
    #[serde(rename = "fileSizeOnDisk", default)]
    pub file_size_on_disk: Option<i64>,
    #[serde(rename = "downloadUrl", default)]
    pub download_url: Option<String>,
    #[serde(rename = "gameVersions", default)]
    pub game_versions: Option<Vec<String>>,
    #[serde(rename = "sortableGameVersions", default)]
    pub sortable_game_versions: Option<Vec<FileSortableGameVersions>>,
    #[serde(default)]
    pub dependencies: Option<Vec<FileDependencies>>,
    #[serde(rename = "exposeAsAlternative", default)]
    pub expose_as_alternative: Option<bool>,
    #[serde(rename = "parentProjectFileId", default)]
    pub parent_project_file_id: Option<i32>,
    #[serde(rename = "alternateFileId", default)]
    pub alternate_file_id: Option<i32>,
    #[serde(rename = "isServerPack", default)]
    pub is_server_pack: Option<bool>,
    #[serde(rename = "serverPackFileId", default)]
    pub server_pack_file_id: Option<i32>,
    #[serde(rename = "isEarlyAccessContent", default)]
    pub is_early_access_content: Option<bool>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "earlyAccessEndDate", default)]
    pub early_access_end_date: Option<DateTime<Utc>>,
    #[serde(rename = "fileFingerprint", default)]
    pub file_fingerprint: Option<i64>,
    #[serde(default)]
    pub modules: Option<Vec<Module>>,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `Mod.latestFiles` 内嵌的文件，与 `File` 的区别是没有 `_id` 与 `sync_at`
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub id: i32,
    #[serde(rename = "gameId")]
    pub game_id: i32,
    #[serde(rename = "modId")]
    pub mod_id: i32,
    #[serde(rename = "isAvailable", default)]
    pub is_available: Option<bool>,
    #[serde(rename = "displayName", default)]
    pub display_name: Option<String>,
    #[serde(rename = "fileName", default)]
    pub file_name: Option<String>,
    #[serde(rename = "releaseType", default)]
    pub release_type: Option<i32>,
    #[serde(rename = "fileStatus", default)]
    pub file_status: Option<i32>,
    #[serde(default)]
    pub hashes: Option<Vec<Hash>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "fileDate", default)]
    pub file_date: Option<DateTime<Utc>>,
    #[serde(rename = "fileLength", default)]
    pub file_length: Option<i64>,
    #[serde(rename = "downloadCount", default)]
    pub download_count: Option<i64>,
    #[serde(rename = "fileSizeOnDisk", default)]
    pub file_size_on_disk: Option<i64>,
    #[serde(rename = "downloadUrl", default)]
    pub download_url: Option<String>,
    #[serde(rename = "gameVersions", default)]
    pub game_versions: Option<Vec<String>>,
    #[serde(rename = "sortableGameVersions", default)]
    pub sortable_game_versions: Option<Vec<FileSortableGameVersions>>,
    #[serde(default)]
    pub dependencies: Option<Vec<FileDependencies>>,
    #[serde(rename = "exposeAsAlternative", default)]
    pub expose_as_alternative: Option<bool>,
    #[serde(rename = "parentProjectFileId", default)]
    pub parent_project_file_id: Option<i32>,
    #[serde(rename = "alternateFileId", default)]
    pub alternate_file_id: Option<i32>,
    #[serde(rename = "isServerPack", default)]
    pub is_server_pack: Option<bool>,
    #[serde(rename = "serverPackFileId", default)]
    pub server_pack_file_id: Option<i32>,
    #[serde(rename = "isEarlyAccessContent", default)]
    pub is_early_access_content: Option<bool>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "earlyAccessEndDate", default)]
    pub early_access_end_date: Option<DateTime<Utc>>,
    #[serde(rename = "fileFingerprint", default)]
    pub file_fingerprint: Option<i64>,
    #[serde(default)]
    pub modules: Option<Vec<Module>>,
}

/// `curseforge_mods` 集合
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mod {
    #[serde(rename = "_id", alias = "id")]
    pub id: i32,
    #[serde(rename = "gameId", default)]
    pub game_id: Option<i32>,
    #[serde(default)]
    pub name: Option<String>,
    pub slug: String,
    #[serde(default)]
    pub links: Option<Links>,
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub status: Option<i32>,
    #[serde(rename = "downloadCount", default)]
    pub download_count: Option<i64>,
    #[serde(rename = "isFeatured", default)]
    pub is_featured: Option<bool>,
    #[serde(rename = "primaryCategoryId", default)]
    pub primary_category_id: Option<i32>,
    #[serde(default)]
    pub categories: Option<Vec<CategoryInfo>>,
    #[serde(rename = "classId", default)]
    pub class_id: Option<i32>,
    #[serde(default)]
    pub authors: Option<Vec<Author>>,
    #[serde(default)]
    pub logo: Option<Logo>,
    #[serde(default)]
    pub screenshots: Option<Vec<ScreenShot>>,
    #[serde(rename = "mainFileId", default)]
    pub main_file_id: Option<i32>,
    #[serde(rename = "latestFiles", default)]
    pub latest_files: Option<Vec<FileInfo>>,
    #[serde(rename = "latestFilesIndexes", default)]
    pub latest_files_indexes: Option<Vec<FileIndex>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "dateCreated", default)]
    pub date_created: Option<DateTime<Utc>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "dateModified", default)]
    pub date_modified: Option<DateTime<Utc>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(rename = "dateReleased", default)]
    pub date_released: Option<DateTime<Utc>>,
    #[serde(rename = "allowModDistribution", default)]
    pub allow_mod_distribution: Option<bool>,
    #[serde(rename = "gamePopularityRank", default)]
    pub game_popularity_rank: Option<i32>,
    #[serde(rename = "isAvailable", default)]
    pub is_available: Option<bool>,
    #[serde(rename = "thumbsUpCount", default)]
    pub thumbs_up_count: Option<i32>,
    #[serde(default)]
    pub rating: Option<i32>,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `curseforge_categories` 集合
///
/// `displayIndex` 在 mcim-rust-api 侧是必填 i32，缺失时回落到 0，
/// 否则写入 null 会让读侧反序列化失败
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    #[serde(rename = "_id", alias = "id")]
    pub id: i32,
    #[serde(rename = "gameId")]
    pub game_id: i32,
    pub name: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(rename = "iconUrl", default)]
    pub icon_url: Option<String>,
    #[serde_as(as = "FlexDateTime")]
    #[serde(rename = "dateModified")]
    pub date_modified: DateTime<Utc>,
    #[serde(rename = "isClass", default)]
    pub is_class: Option<bool>,
    #[serde(rename = "classId", default)]
    pub class_id: Option<i32>,
    #[serde(rename = "parentCategoryId", default)]
    pub parent_category_id: Option<i32>,
    #[serde(rename = "displayIndex", default)]
    pub display_index: i32,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}
