pub mod curseforge;
pub mod modrinth;
pub mod translate;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_with::{DeserializeAs, SerializeAs};

/// MongoDB 集合名
pub mod collection {
    pub const CURSEFORGE_MODS: &str = "curseforge_mods";
    pub const CURSEFORGE_FILES: &str = "curseforge_files";
    pub const CURSEFORGE_CATEGORIES: &str = "curseforge_categories";
    pub const CURSEFORGE_TRANSLATED: &str = "curseforge_translated";

    pub const MODRINTH_PROJECTS: &str = "modrinth_projects";
    pub const MODRINTH_VERSIONS: &str = "modrinth_versions";
    pub const MODRINTH_FILES: &str = "modrinth_files";
    pub const MODRINTH_CATEGORIES: &str = "modrinth_categories";
    pub const MODRINTH_LOADERS: &str = "modrinth_loaders";
    pub const MODRINTH_GAME_VERSIONS: &str = "modrinth_game_versions";
    pub const MODRINTH_TRANSLATED: &str = "modrinth_translated";
}

/// 同一个结构体既要吃上游 API 的 RFC3339 字符串，又要吃库里的 BSON DateTime，
/// 写回时统一转成 BSON DateTime，保证 mcim-rust-api 能读
pub struct FlexDateTime;

#[derive(Deserialize)]
#[serde(untagged)]
enum AnyDateTime {
    Bson(bson::DateTime),
    Chrono(DateTime<Utc>),
}

impl SerializeAs<DateTime<Utc>> for FlexDateTime {
    fn serialize_as<S>(source: &DateTime<Utc>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        bson::DateTime::from_chrono(*source).serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, DateTime<Utc>> for FlexDateTime {
    fn deserialize_as<D>(deserializer: D) -> Result<DateTime<Utc>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match AnyDateTime::deserialize(deserializer)? {
            AnyDateTime::Bson(value) => value.to_chrono(),
            AnyDateTime::Chrono(value) => value,
        })
    }
}

/// `sync_at` 缺省值，上游响应里没有这个字段
pub fn now() -> DateTime<Utc> {
    Utc::now()
}
