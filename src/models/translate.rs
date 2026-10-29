use chrono::{DateTime, Utc};
use sqlx::FromRow;

/// 本仓库只负责标记 need_to_update，实际翻译由 mcim-translate 完成
#[derive(Debug, Clone, FromRow)]
pub struct CurseForgeTranslation {
    pub mod_id: i32,
    pub translated: Option<String>,
    pub original: Option<String>,
    pub need_to_update: bool,
    pub translated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, FromRow)]
pub struct ModrinthTranslation {
    pub project_id: String,
    pub translated: Option<String>,
    pub original: Option<String>,
    pub need_to_update: bool,
    pub translated_at: Option<DateTime<Utc>>,
}
