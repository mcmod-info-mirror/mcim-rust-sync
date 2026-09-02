use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::{FlexDateTime, now};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DonationUrl {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct License {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GalleryItem {
    pub url: String,
    pub featured: bool,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde_as(as = "FlexDateTime")]
    pub created: DateTime<Utc>,
    #[serde(default)]
    pub ordering: Option<i64>,
}

/// `modrinth_projects` 集合
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    #[serde(rename = "_id", alias = "id")]
    pub id: String,
    pub slug: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub categories: Option<Vec<String>>,
    #[serde(default)]
    pub client_side: Option<String>,
    #[serde(default)]
    pub server_side: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub requested_status: Option<String>,
    #[serde(default)]
    pub additional_categories: Option<Vec<String>>,
    #[serde(default)]
    pub issues_url: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub wiki_url: Option<String>,
    #[serde(default)]
    pub discord_url: Option<String>,
    #[serde(default)]
    pub donation_urls: Option<Vec<DonationUrl>>,
    #[serde(default)]
    pub project_type: Option<String>,
    #[serde(default)]
    pub downloads: Option<i64>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub color: Option<u32>,
    #[serde(default)]
    pub thread_id: Option<String>,
    #[serde(default)]
    pub monetization_status: Option<String>,
    pub team: String,
    #[serde(default)]
    pub body_url: Option<String>,
    #[serde_as(as = "FlexDateTime")]
    pub published: DateTime<Utc>,
    #[serde_as(as = "FlexDateTime")]
    pub updated: DateTime<Utc>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default)]
    pub approved: Option<DateTime<Utc>>,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default)]
    pub queued: Option<DateTime<Utc>>,
    pub followers: u32,
    #[serde(default)]
    pub license: Option<License>,
    #[serde(default)]
    pub versions: Option<Vec<String>>,
    #[serde(default)]
    pub game_versions: Option<Vec<String>>,
    #[serde(default)]
    pub loaders: Option<Vec<String>>,
    #[serde(default)]
    pub gallery: Option<Vec<GalleryItem>>,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,

    /// 最后一次核对过上游、确认这条仍是最新的时刻
    ///
    /// 与 `sync_at` 的区别：内容没变也会更新。只有 `sync_at` 的话，
    /// 「从没检查过」和「刚确认过但没变」这两种情况分不开
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependencies {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    pub dependency_type: String,
}

/// `modrinth_files` 的 `_id`，字段顺序必须是 sha512 在前
///
/// MongoDB 对子文档主键的相等性是字段顺序敏感的，换序会凭空多出一份文件文档
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hashes {
    pub sha512: String,
    pub sha1: String,
}

/// `modrinth_files` 集合
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    #[serde(rename = "_id", alias = "hashes")]
    pub hashes: Hashes,
    pub url: String,
    pub filename: String,
    pub primary: bool,
    pub size: i64,
    #[serde(default)]
    pub file_type: Option<String>,
    pub version_id: String,
    pub project_id: String,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `Version.files` 内嵌的文件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub hashes: Hashes,
    pub url: String,
    pub filename: String,
    pub primary: bool,
    pub size: i64,
    #[serde(default)]
    pub file_type: Option<String>,
}

/// `modrinth_versions` 集合
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Version {
    #[serde(rename = "_id", alias = "id")]
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version_number: Option<String>,
    #[serde(default)]
    pub changelog: Option<String>,
    #[serde(default)]
    pub dependencies: Option<Vec<Dependencies>>,
    #[serde(default)]
    pub game_versions: Option<Vec<String>>,
    #[serde(default)]
    pub version_type: Option<String>,
    #[serde(default)]
    pub loaders: Option<Vec<String>>,
    #[serde(default)]
    pub featured: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub requested_status: Option<String>,
    pub author_id: String,
    #[serde_as(as = "FlexDateTime")]
    pub date_published: DateTime<Utc>,
    pub downloads: i64,
    #[serde(default)]
    pub changelog_url: Option<String>,
    pub files: Vec<FileInfo>,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `modrinth_categories` 集合，无主键，每次同步整表替换
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub icon: String,
    pub name: String,
    #[serde(default)]
    pub project_type: Option<String>,
    pub header: String,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `modrinth_loaders` 集合，无主键，每次同步整表替换
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Loader {
    pub icon: String,
    pub name: String,
    pub supported_project_types: Vec<String>,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}

/// `modrinth_game_versions` 集合，无主键，每次同步整表替换
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameVersion {
    pub version: String,
    pub version_type: String,
    #[serde_as(as = "FlexDateTime")]
    pub date: DateTime<Utc>,
    pub major: bool,

    #[serde_as(as = "FlexDateTime")]
    #[serde(default = "now")]
    pub sync_at: DateTime<Utc>,
}
