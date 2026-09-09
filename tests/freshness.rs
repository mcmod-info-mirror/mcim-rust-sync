//! `/api/freshness` 的计数逻辑集成测试
//!
//! 直接走 `Database` 连 MongoDB，不牵涉 Redis。本机没起 MongoDB 就跳过，

use bson::{Document, doc};
use chrono::{DateTime, TimeZone, Utc};
use mcim_rust_sync::config::Config;
use mcim_rust_sync::db::Database;
use mcim_rust_sync::models::collection;

fn config(database: &str) -> Config {
    let raw = format!(
        r#"{{
            "mongodb": {{ "host": "localhost", "port": 27017, "database": "{}" }}
        }}"#,
        database
    );
    Config::from_json(&raw).expect("解析测试配置失败")
}

async fn database(database: &str) -> Option<Database> {
    match Database::connect(&config(database)).await {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("跳过：连不上 MongoDB（{}）", e);
            None
        }
    }
}

async fn reset(db: &Database, names: &[&str]) {
    for name in names {
        db.delete_many(name, doc! {}).await.expect("清库失败");
    }
}

#[tokio::test]
async fn freshness_counts_by_checked_at_windows() {
    let db = match database("mcim_freshness_test").await {
        Some(db) => db,
        None => return,
    };
    let names = [collection::CURSEFORGE_MODS, collection::MODRINTH_PROJECTS];
    reset(&db, &names).await;

    // 固定到毫秒对齐的时间，避免 BSON DateTime 毫秒精度回读后跟预期值差了个尾巴
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

    // curseforge_mods 的 `_id` 是 i32，modrinth_projects 是字符串
    for name in names {
        let docs: Vec<Document> = entries
            .iter()
            .enumerate()
            .map(|(index, (checked, sync))| {
                let mut doc = doc! {
                    "sync_at": bson::DateTime::from_chrono(*sync),
                };
                if name == collection::CURSEFORGE_MODS {
                    doc.insert("_id", (index + 1) as i32);
                } else {
                    doc.insert("_id", format!("proj-{}", index + 1));
                }
                if let Some(value) = checked {
                    doc.insert("checked_at", bson::DateTime::from_chrono(*value));
                }
                doc
            })
            .collect();
        db.collection::<Document>(name)
            .insert_many(docs)
            .await
            .expect("插入新鲜度样本失败");
    }

    let summary = db
        .freshness_summary(now)
        .await
        .expect("freshness_summary 失败");

    for name in names {
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
