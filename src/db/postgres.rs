use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{PgConnection, PgPool};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::models::collection;
use crate::models::task_run::TaskRun;

/// mcim-rust-api 的 migrations 建立的表，缺任何一张都说明库还没初始化
const REQUIRED_TABLES: [&str; 11] = [
    collection::CURSEFORGE_MODS,
    collection::CURSEFORGE_FILES,
    collection::CURSEFORGE_CATEGORIES,
    collection::CURSEFORGE_TRANSLATED,
    collection::MODRINTH_PROJECTS,
    collection::MODRINTH_VERSIONS,
    collection::MODRINTH_FILES,
    collection::MODRINTH_CATEGORIES,
    collection::MODRINTH_LOADERS,
    collection::MODRINTH_GAME_VERSIONS,
    collection::MODRINTH_TRANSLATED,
];

/// 本仓库自己的表：任务运行记录，读写都只在这里
const CREATE_TASK_RUNS: &str = "CREATE TABLE IF NOT EXISTS task_runs (
    id          bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task        text        NOT NULL,
    provider    text        NOT NULL,
    operation   text        NOT NULL,
    status      text        NOT NULL,
    started_at  timestamptz NOT NULL,
    finished_at timestamptz,
    duration_ms bigint,
    total       bigint      NOT NULL,
    synced      bigint      NOT NULL,
    not_found   bigint      NOT NULL,
    skipped     bigint      NOT NULL,
    failed      bigint      NOT NULL,
    requeued    bigint      NOT NULL,
    versions    bigint      NOT NULL,
    files       bigint      NOT NULL,
    discovered  bigint      NOT NULL,
    removed     bigint      NOT NULL,
    error       text,
    version     text        NOT NULL
)";

/// 同步与查询额外需要的索引；mcim-rust-api 的 migrations 只建了 API 读路径用到的
const INDEXES: [(&str, &str); 8] = [
    (
        "curseforge_mods_checked_at_idx",
        "CREATE INDEX IF NOT EXISTS curseforge_mods_checked_at_idx ON curseforge_mods (checked_at)",
    ),
    (
        "curseforge_mods_sync_at_idx",
        "CREATE INDEX IF NOT EXISTS curseforge_mods_sync_at_idx ON curseforge_mods (sync_at)",
    ),
    (
        "modrinth_projects_checked_at_idx",
        "CREATE INDEX IF NOT EXISTS modrinth_projects_checked_at_idx ON modrinth_projects (checked_at)",
    ),
    (
        "modrinth_projects_sync_at_idx",
        "CREATE INDEX IF NOT EXISTS modrinth_projects_sync_at_idx ON modrinth_projects (sync_at)",
    ),
    (
        "modrinth_files_project_version_idx",
        "CREATE INDEX IF NOT EXISTS modrinth_files_project_version_idx \
         ON modrinth_files (project_id, version_id)",
    ),
    (
        "modrinth_files_version_id_idx",
        "CREATE INDEX IF NOT EXISTS modrinth_files_version_id_idx ON modrinth_files (version_id)",
    ),
    (
        "task_runs_task_started_at_idx",
        "CREATE INDEX IF NOT EXISTS task_runs_task_started_at_idx ON task_runs (task, started_at DESC)",
    ),
    (
        "task_runs_status_started_at_idx",
        "CREATE INDEX IF NOT EXISTS task_runs_status_started_at_idx \
         ON task_runs (status, started_at DESC)",
    ),
];

#[derive(Clone)]
pub struct Database {
    pub(crate) pool: PgPool,
}

/// `GET /api/freshness` 的响应体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessResponse {
    pub generated_at: DateTime<Utc>,
    pub collections: BTreeMap<String, FreshnessCollection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessCollection {
    pub total: u64,
    pub checked_within: CheckedWithin,
    pub never_checked: u64,
    pub oldest_checked_at: Option<DateTime<Utc>>,
    pub newest_sync_at: Option<DateTime<Utc>>,
}

/// 键是 `2h` / `24h` / `7d`，不是合法 Rust 标识符，所以用手写 rename
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckedWithin {
    #[serde(rename = "2h")]
    pub h2: u64,
    #[serde(rename = "24h")]
    pub h24: u64,
    #[serde(rename = "7d")]
    pub d7: u64,
}

/// 连接串里去掉密码后的连接目标，只用于日志
pub fn describe_target(database_url: &str) -> String {
    match PgConnectOptions::from_str(database_url) {
        Ok(options) => format!(
            "{}:{}/{}",
            options.get_host(),
            options.get_port(),
            options.get_database().unwrap_or_default()
        ),
        Err(_) => "<invalid DATABASE_URL>".to_string(),
    }
}

impl Database {
    pub async fn connect(config: &Config) -> Result<Self> {
        let options =
            PgConnectOptions::from_str(&config.database_url)?.application_name("mcim-rust-sync");
        // 每个任务内最多 max_workers 个并发写入，守护模式下还有多个任务并行
        let pool = PgPoolOptions::new()
            .max_connections((config.max_workers as u32).saturating_mul(2).max(4))
            .connect_with(options)
            .await?;

        let missing: Vec<String> = sqlx::query_scalar(
            "SELECT name FROM unnest($1::text[]) AS name WHERE to_regclass(name) IS NULL",
        )
        .bind(&REQUIRED_TABLES[..])
        .fetch_all(&pool)
        .await?;
        if !missing.is_empty() {
            return Err(Error::Config(format!(
                "PostgreSQL 里缺少 {}，表结构由 mcim-rust-api 启动时建立，请先启动它",
                missing.join(", ")
            )));
        }

        sqlx::query(CREATE_TASK_RUNS).execute(&pool).await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// 开启写事务，并按 `keys` 对文档加事务级咨询锁
    ///
    /// 一份文档分布在多张表里，替换是「先删后插」；守护模式下不同任务可能同时
    /// 同步同一个条目，不加锁的话后提交的一方会撞上主键冲突
    pub(crate) async fn begin_locked(
        &self,
        namespace: &str,
        keys: Vec<String>,
    ) -> Result<sqlx::Transaction<'static, sqlx::Postgres>> {
        let mut tx = self.pool.begin().await?;
        lock_documents(&mut tx, namespace, keys).await?;
        Ok(tx)
    }

    pub async fn insert_task_run(&self, run: &mut TaskRun) -> Result<()> {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO task_runs (task, provider, operation, status, started_at, finished_at, \
             duration_ms, total, synced, not_found, skipped, failed, requeued, versions, files, \
             discovered, removed, error, version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, \
             $17, $18, $19) RETURNING id",
        )
        .bind(&run.task)
        .bind(&run.provider)
        .bind(&run.operation)
        .bind(&run.status)
        .bind(run.started_at)
        .bind(run.finished_at)
        .bind(run.duration_ms)
        .bind(run.total)
        .bind(run.synced)
        .bind(run.not_found)
        .bind(run.skipped)
        .bind(run.failed)
        .bind(run.requeued)
        .bind(run.versions)
        .bind(run.files)
        .bind(run.discovered)
        .bind(run.removed)
        .bind(&run.error)
        .bind(&run.version)
        .fetch_one(&self.pool)
        .await?;
        run.id = Some(id);
        Ok(())
    }

    /// 没有 id 说明开始记录没写进去，结束时补写一条完整记录
    pub async fn update_task_run(&self, run: &mut TaskRun) -> Result<()> {
        let Some(id) = run.id else {
            return self.insert_task_run(run).await;
        };
        sqlx::query(
            "UPDATE task_runs SET status = $2, finished_at = $3, duration_ms = $4, total = $5, \
             synced = $6, not_found = $7, skipped = $8, failed = $9, requeued = $10, \
             versions = $11, files = $12, discovered = $13, removed = $14, error = $15 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(&run.status)
        .bind(run.finished_at)
        .bind(run.duration_ms)
        .bind(run.total)
        .bind(run.synced)
        .bind(run.not_found)
        .bind(run.skipped)
        .bind(run.failed)
        .bind(run.requeued)
        .bind(run.versions)
        .bind(run.files)
        .bind(run.discovered)
        .bind(run.removed)
        .bind(&run.error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_running_task_runs_interrupted(
        &self,
        finished_at: DateTime<Utc>,
    ) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE task_runs SET status = 'interrupted', finished_at = $1, \
             error = 'process restarted before task completion' WHERE status = 'running'",
        )
        .bind(finished_at)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    pub async fn list_task_runs(
        &self,
        task: Option<&str>,
        status: Option<&str>,
        started_after: Option<DateTime<Utc>>,
        started_before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<TaskRun>> {
        Ok(sqlx::query_as(
            "SELECT id, task, provider, operation, status, started_at, finished_at, duration_ms, \
             total, synced, not_found, skipped, failed, requeued, versions, files, discovered, \
             removed, error, version FROM task_runs \
             WHERE ($1::text IS NULL OR task = $1) \
             AND ($2::text IS NULL OR status = $2) \
             AND ($3::timestamptz IS NULL OR started_at >= $3) \
             AND ($4::timestamptz IS NULL OR started_at <= $4) \
             ORDER BY started_at DESC, id DESC LIMIT $5",
        )
        .bind(task)
        .bind(status)
        .bind(started_after)
        .bind(started_before)
        .bind(limit.clamp(1, 500))
        .fetch_all(&self.pool)
        .await?)
    }

    pub async fn ensure_indexes(&self) -> Result<Vec<String>> {
        let mut created = Vec::with_capacity(INDEXES.len());
        for (name, sql) in INDEXES {
            sqlx::query(sql).execute(&self.pool).await?;
            created.push(name.to_string());
        }
        Ok(created)
    }

    /// 两个主表的新鲜度汇总，两张表并行统计
    pub async fn freshness_summary(&self, now: DateTime<Utc>) -> Result<FreshnessResponse> {
        let (curseforge, modrinth) = tokio::join!(
            self.freshness(
                "SELECT count(*), \
                 count(*) FILTER (WHERE checked_at >= $1), \
                 count(*) FILTER (WHERE checked_at >= $2), \
                 count(*) FILTER (WHERE checked_at >= $3), \
                 count(checked_at), min(checked_at), max(sync_at) FROM curseforge_mods",
                now
            ),
            self.freshness(
                "SELECT count(*), \
                 count(*) FILTER (WHERE checked_at >= $1), \
                 count(*) FILTER (WHERE checked_at >= $2), \
                 count(*) FILTER (WHERE checked_at >= $3), \
                 count(checked_at), min(checked_at), max(sync_at) FROM modrinth_projects",
                now
            ),
        );

        let mut collections = BTreeMap::new();
        collections.insert(collection::CURSEFORGE_MODS.to_string(), curseforge?);
        collections.insert(collection::MODRINTH_PROJECTS.to_string(), modrinth?);

        Ok(FreshnessResponse {
            generated_at: now,
            collections,
        })
    }

    /// 单张表的「核对新鲜度」统计，一次扫描算完全部计数
    async fn freshness(
        &self,
        sql: &'static str,
        now: DateTime<Utc>,
    ) -> Result<FreshnessCollection> {
        #[allow(clippy::type_complexity)]
        let (total, h2, h24, d7, checked, oldest_checked_at, newest_sync_at): (
            i64,
            i64,
            i64,
            i64,
            i64,
            Option<DateTime<Utc>>,
            Option<DateTime<Utc>>,
        ) = sqlx::query_as(sql)
            .bind(now - chrono::Duration::hours(2))
            .bind(now - chrono::Duration::hours(24))
            .bind(now - chrono::Duration::days(7))
            .fetch_one(&self.pool)
            .await?;

        Ok(FreshnessCollection {
            total: total as u64,
            checked_within: CheckedWithin {
                h2: h2 as u64,
                h24: h24 as u64,
                d7: d7 as u64,
            },
            never_checked: (total - checked) as u64,
            oldest_checked_at,
            newest_sync_at,
        })
    }
}

/// 按固定顺序加锁，两个事务锁同一批键时不会互相死锁
async fn lock_documents(
    conn: &mut PgConnection,
    namespace: &str,
    mut keys: Vec<String>,
) -> Result<()> {
    keys.sort_unstable();
    keys.dedup();
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended($1 || ':' || key, 0)) \
         FROM unnest($2::text[]) WITH ORDINALITY AS k(key, ord) ORDER BY ord",
    )
    .bind(namespace)
    .bind(keys)
    .execute(conn)
    .await?;
    Ok(())
}

/// 将 `parent -> [value]` 展开为 UNNEST 所需的 (父键, position, 值) 三列
pub(crate) fn flatten_string_lists<'a, K: Clone + 'a>(
    lists: impl IntoIterator<Item = (K, Option<&'a Vec<String>>)>,
) -> (Vec<K>, Vec<i32>, Vec<&'a str>) {
    let mut parents = Vec::new();
    let mut positions = Vec::new();
    let mut values = Vec::new();
    for (parent, list) in lists {
        for (position, value) in list.into_iter().flatten().enumerate() {
            parents.push(parent.clone());
            positions.push(position as i32);
            values.push(value.as_str());
        }
    }
    (parents, positions, values)
}

/// 为自增主键表预分配 `count` 个 id，便于在一次批量写入中同时写入子表
pub(crate) async fn allocate_ids(
    conn: &mut PgConnection,
    sequence_sql: &'static str,
    count: usize,
) -> Result<Vec<i64>> {
    if count == 0 {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(sequence_sql)
        .bind(count as i64)
        .fetch_all(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_serializes_with_contract_keys() {
        let now = Utc::now();
        let mut collections = BTreeMap::new();
        collections.insert(
            collection::CURSEFORGE_MODS.to_string(),
            FreshnessCollection {
                total: 3,
                checked_within: CheckedWithin {
                    h2: 1,
                    h24: 2,
                    d7: 3,
                },
                never_checked: 1,
                oldest_checked_at: Some(now),
                newest_sync_at: Some(now),
            },
        );

        let value = serde_json::to_value(FreshnessResponse {
            generated_at: now,
            collections,
        })
        .expect("freshness 响应可序列化");

        let item = &value["collections"]["curseforge_mods"];
        assert_eq!(item["total"], 3);
        assert_eq!(item["never_checked"], 1);
        assert_eq!(item["checked_within"]["2h"], 1);
        assert_eq!(item["checked_within"]["24h"], 2);
        assert_eq!(item["checked_within"]["7d"], 3);
        assert!(item["oldest_checked_at"].is_string());
        assert!(item["newest_sync_at"].is_string());
    }

    #[test]
    fn describe_target_hides_password() {
        let target = describe_target("postgresql://mcim:secret@db.internal:5433/mcim_backend");
        assert_eq!(target, "db.internal:5433/mcim_backend");
    }
}
