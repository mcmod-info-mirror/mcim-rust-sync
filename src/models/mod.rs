pub mod curseforge;
pub mod modrinth;
pub mod task_run;
pub mod translate;

use chrono::{DateTime, Utc};

/// PostgreSQL 主表名，表结构由 mcim-rust-api 的 migrations 维护
pub mod collection {
    pub const TASK_RUNS: &str = "task_runs";
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

/// `sync_at` 缺省值，上游响应里没有这个字段
pub fn now() -> DateTime<Utc> {
    Utc::now()
}
