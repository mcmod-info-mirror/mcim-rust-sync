//! `/api/freshness` 的计数逻辑集成测试
//!
//! 直接走 `Database` 连 PostgreSQL，不牵涉 Redis。没设 `DATABASE_URL` 就跳过，

mod common;

use chrono::{DateTime, TimeZone, Utc};
use mcim_rust_sync::config::Config;
use mcim_rust_sync::db::Database;
use mcim_rust_sync::models::collection;

async fn database(name: &str) -> Option<Database> {
    let url = common::fresh_database(name).await?;
    let mut config = Config::from_json("{}").expect("解析测试配置失败");
    config.database_url = url;
    Some(Database::connect(&config).await.expect("连接测试库失败"))
}

#[tokio::test]
async fn freshness_counts_by_checked_at_windows() {
    let Some(db) = database("mcim_freshness_test").await else {
        return;
    };

    let now = Utc
        .with_ymd_and_hms(2026, 9, 9, 12, 0, 0)
        .single()
        .expect("合法时间");

    // (checked_at, sync_at)：2h 内、2h~24h、24h~7d、从未核对、超过 7d
    let entries: [(Option<DateTime<Utc>>, DateTime<Utc>); 5] = [
        (
            Some(now - chrono::Duration::hours(1)),
            now - chrono::Duration::minutes(30), // 最新 sync_at
        ),
        (
            Some(now - chrono::Duration::hours(5)),
            now - chrono::Duration::hours(5),
        ),
        (
            Some(now - chrono::Duration::days(3)),
            now - chrono::Duration::days(3),
        ),
        (None, now - chrono::Duration::days(4)),
        (
            Some(now - chrono::Duration::days(10)),
            now - chrono::Duration::days(10),
        ),
    ];

    for (index, (checked, sync)) in entries.iter().enumerate() {
        sqlx::query(
            "INSERT INTO curseforge_mods (id, slug, sync_at, checked_at) VALUES ($1, $2, $3, $4)",
        )
        .bind((index + 1) as i32)
        .bind(format!("mod-{}", index + 1))
        .bind(sync)
        .bind(checked)
        .execute(db.pool())
        .await
        .expect("插入 curseforge_mods 样本失败");
        sqlx::query(
            "INSERT INTO modrinth_projects (id, slug, team, published, updated, followers, \
             sync_at, checked_at) VALUES ($1, $1, 'team', $2, $2, 0, $2, $3)",
        )
        .bind(format!("proj-{}", index + 1))
        .bind(sync)
        .bind(checked)
        .execute(db.pool())
        .await
        .expect("插入 modrinth_projects 样本失败");
    }

    let summary = db
        .freshness_summary(now)
        .await
        .expect("freshness_summary 失败");

    for name in [collection::CURSEFORGE_MODS, collection::MODRINTH_PROJECTS] {
        let item = &summary.collections[name];
        assert_eq!(item.total, 5, "{name} total");
        assert_eq!(item.checked_within.h2, 1, "{name} 2h");
        assert_eq!(item.checked_within.h24, 2, "{name} 24h");
        assert_eq!(item.checked_within.d7, 3, "{name} 7d");
        assert_eq!(item.never_checked, 1, "{name} never_checked");
        assert_eq!(
            item.oldest_checked_at,
            Some(now - chrono::Duration::days(10)),
            "{name} oldest_checked_at"
        );
        assert_eq!(
            item.newest_sync_at,
            Some(now - chrono::Duration::minutes(30)),
            "{name} newest_sync_at"
        );
    }
}
