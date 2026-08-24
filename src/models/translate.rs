use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use super::FlexDateTime;

/// 本仓库只负责标记 need_to_update，实际翻译由 mcim-translate 完成
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurseForgeTranslation {
    #[serde(rename = "_id", alias = "modId")]
    pub mod_id: i32,
    #[serde(default)]
    pub translated: Option<String>,
    #[serde(default)]
    pub original: Option<String>,
    pub need_to_update: bool,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default)]
    pub translated_at: Option<DateTime<Utc>>,
}

#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModrinthTranslation {
    #[serde(rename = "_id", alias = "project_id")]
    pub project_id: String,
    #[serde(default)]
    pub translated: Option<String>,
    #[serde(default)]
    pub original: Option<String>,
    pub need_to_update: bool,
    #[serde_as(as = "Option<FlexDateTime>")]
    #[serde(default)]
    pub translated_at: Option<DateTime<Utc>>,
}
