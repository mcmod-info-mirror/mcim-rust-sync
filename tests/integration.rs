//! 打真实 MongoDB 与 Redis 的集成测试
//!
//! 验证的是「写进去的东西 mcim-rust-api 读得出来」以及队列的原子语义。
//! 本机没起这两个服务时直接跳过，不让 `cargo test` 失败。

use bson::{Document, doc};
use chrono::{Duration, Utc};
use mcim_rust_sync::config::Config;
use mcim_rust_sync::db::queue::key;
use mcim_rust_sync::db::{Database, Queues};
use mcim_rust_sync::models::curseforge as cf;
use mcim_rust_sync::models::collection;
use mcim_rust_sync::models::modrinth as mr;
use mcim_rust_sync::models::translate::ModrinthTranslation;

/// 测试用配置，库名与队列 key 都带前缀，不碰真实数据也不落盘
fn test_config(database: &str) -> Config {
    let raw = format!(
        r#"{{
            "mongodb": {{ "host": "localhost", "port": 27017, "database": "{}" }},
            "redis": {{ "host": "localhost", "port": 6379, "database": 15 }},
            "curseforge_api_key": "test"
        }}"#,
        database
    );
    Config::from_json(&raw).expect("解析测试配置失败")
}

async fn database(name: &str) -> Option<Database> {
    match Database::connect(&test_config(name)).await {
        Ok(db) => Some(db),
        Err(e) => {
            eprintln!("跳过：连不上 MongoDB ({})", e);
            None
        }
    }
}

async fn queues() -> Option<Queues> {
    match Queues::connect(&test_config("mcim_test_queue")).await {
        Ok(q) => Some(q),
        Err(e) => {
            eprintln!("跳过：连不上 Redis ({})", e);
            None
        }
    }
}

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    let raw = std::fs::read_to_string(&path).expect("读取夹具失败");
    serde_json::from_str(&raw).expect("解析夹具失败")
}

/// 写进 MongoDB 之后，`_id` 与各时间字段必须是 mcim-rust-api 期望的 BSON 类型
#[tokio::test]
async fn writes_bson_types_readable_by_api() {
    let Some(db) = database("mcim_test_types").await else {
        return;
    };
    let collection = "curseforge_mods";
    db.delete_many(collection, doc! {}).await.unwrap();

    let mods: Vec<cf::Mod> = load("db_curseforge_mods.json");
    db.upsert_many(collection, &mods, 4).await.unwrap();

    let stored: Document = db
        .collection::<Document>(collection)
        .find_one(doc! { "_id": mods[0].id })
        .await
        .unwrap()
        .expect("写入的文档没找到");

    assert!(matches!(stored.get("_id"), Some(bson::Bson::Int32(_))));
    assert!(matches!(stored.get("sync_at"), Some(bson::Bson::DateTime(_))));
    assert!(matches!(
        stored.get("dateModified"),
        Some(bson::Bson::DateTime(_))
    ));
    // 嵌套结构里的时间同样要归一成 BSON DateTime
    let nested = stored.get_array("categories").unwrap()[0]
        .as_document()
        .unwrap();
    assert!(matches!(
        nested.get("dateModified"),
        Some(bson::Bson::DateTime(_))
    ));
    // 遗留字段不再写回
    assert!(!stored.contains_key("file_cdn_cached"));
    assert!(!stored.contains_key("found"));

    db.delete_many(collection, doc! {}).await.unwrap();
}

/// 同一批数据写两次不能变成两份
#[tokio::test]
async fn upsert_is_idempotent() {
    let Some(db) = database("mcim_test_upsert").await else {
        return;
    };
    let collection = "modrinth_files";
    db.delete_many(collection, doc! {}).await.unwrap();

    let files: Vec<mr::File> = load("db_modrinth_files.json");
    db.upsert_many(collection, &files, 4).await.unwrap();
    let first = db.count(collection).await.unwrap();
    db.upsert_many(collection, &files, 4).await.unwrap();
    let second = db.count(collection).await.unwrap();

    assert_eq!(first, files.len() as u64);
    assert_eq!(first, second, "重复写入产生了多余文档");

    db.delete_many(collection, doc! {}).await.unwrap();
}

/// `modrinth_files` 的主键是子文档，字段顺序换了 MongoDB 就当成另一条
#[tokio::test]
async fn embedded_id_keeps_field_order() {
    let Some(db) = database("mcim_test_id_order").await else {
        return;
    };
    let collection = "modrinth_files";
    db.delete_many(collection, doc! {}).await.unwrap();

    let files: Vec<mr::File> = load("db_modrinth_files.json");
    db.upsert_many(collection, &files[..1], 1).await.unwrap();

    let stored: Document = db
        .collection::<Document>(collection)
        .find_one(doc! {})
        .await
        .unwrap()
        .expect("没写进去");
    let id = stored.get_document("_id").expect("_id 不是子文档");
    let keys: Vec<&str> = id.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["sha512", "sha1"]);

    // 按同样顺序构造的 _id 能查到，说明写入与查询是一致的
    let found = db
        .collection::<Document>(collection)
        .find_one(doc! { "_id": { "sha512": &files[0].hashes.sha512, "sha1": &files[0].hashes.sha1 } })
        .await
        .unwrap();
    assert!(found.is_some(), "按 _id 查不回来");

    db.delete_many(collection, doc! {}).await.unwrap();
}

/// 字典表刷新必须先写新的再删旧的，中途读方不能看到空集合
#[tokio::test]
async fn refresh_collection_never_empties() {
    let Some(db) = database("mcim_test_refresh").await else {
        return;
    };
    let collection = "modrinth_loaders";
    db.delete_many(collection, doc! {}).await.unwrap();

    let mut loaders: Vec<mr::Loader> = load("db_modrinth_loaders.json");
    let old_stamp = Utc::now() - Duration::hours(1);
    for item in loaders.iter_mut() {
        item.sync_at = old_stamp;
    }
    db.refresh_collection(collection, &loaders, old_stamp)
        .await
        .unwrap();
    assert_eq!(db.count(collection).await.unwrap(), loaders.len() as u64);

    // 第二轮：新数据带新时间戳，旧的应当被清掉且总数不变
    let new_stamp = Utc::now();
    for item in loaders.iter_mut() {
        item.sync_at = new_stamp;
    }
    db.refresh_collection(collection, &loaders, new_stamp)
        .await
        .unwrap();
    assert_eq!(
        db.count(collection).await.unwrap(),
        loaders.len() as u64,
        "刷新后条数不对，旧数据没删干净或新数据没进去"
    );

    // 空数据要被拒绝，否则一次上游故障就会清空整张表
    let empty: Vec<mr::Loader> = Vec::new();
    assert!(
        db.refresh_collection(collection, &empty, Utc::now())
            .await
            .is_err()
    );
    assert_eq!(db.count(collection).await.unwrap(), loaders.len() as u64);

    db.delete_many(collection, doc! {}).await.unwrap();
}

/// drain 用 SPOP 原子取走，取走期间新写入的成员必须留在队列里
#[tokio::test]
async fn drain_does_not_swallow_concurrent_writes() {
    let Some(queues) = queues().await else {
        return;
    };
    let queue = "mcim_test_drain";
    let _ = queues.drain(queue).await;

    let first: Vec<String> = (0..50).map(|i| format!("a{}", i)).collect();
    queues.push(queue, &first).await.unwrap();

    let drained = queues.drain(queue).await.unwrap();
    assert_eq!(drained.len(), 50);
    assert_eq!(queues.len(queue).await.unwrap(), 0);

    // drain 之后写入的属于下一轮
    let second: Vec<String> = (0..7).map(|i| format!("b{}", i)).collect();
    queues.push(queue, &second).await.unwrap();
    assert_eq!(queues.len(queue).await.unwrap(), 7);

    let again = queues.drain(queue).await.unwrap();
    assert_eq!(again.len(), 7);
    assert!(again.iter().all(|v| v.starts_with('b')));
}

/// 超过一次 SPOP 批量上限也要取干净
#[tokio::test]
async fn drain_handles_more_than_one_batch() {
    let Some(queues) = queues().await else {
        return;
    };
    let queue = "mcim_test_drain_large";
    let _ = queues.drain(queue).await;

    let members: Vec<String> = (0..2500).map(|i| format!("m{}", i)).collect();
    queues.push(queue, &members).await.unwrap();

    let drained = queues.drain(queue).await.unwrap();
    assert_eq!(drained.len(), 2500);
    assert_eq!(queues.len(queue).await.unwrap(), 0);
}

/// 失败的 id 放回队列后下一轮还能取到
#[tokio::test]
async fn requeue_round_trips() {
    let Some(queues) = queues().await else {
        return;
    };
    let queue = key::CURSEFORGE_MODIDS;
    let _ = queues.drain(queue).await;

    queues
        .push(queue, &["111".to_string(), "222".to_string()])
        .await
        .unwrap();
    let mut drained = queues.drain(queue).await.unwrap();
    drained.sort();
    assert_eq!(drained, vec!["111".to_string(), "222".to_string()]);

    // 只把失败的那个放回去
    queues.push(queue, &["222".to_string()]).await.unwrap();
    assert_eq!(queues.drain(queue).await.unwrap(), vec!["222".to_string()]);
}

/// 索引必须真的建出来，且可以重复执行
#[tokio::test]
async fn ensure_indexes_is_idempotent() {
    let Some(db) = database("mcim_test_indexes").await else {
        return;
    };
    let first = db.ensure_indexes().await.unwrap();
    let second = db.ensure_indexes().await.unwrap();
    assert_eq!(first, second, "重复建索引结果不一致");
    assert!(first.iter().any(|n| n == "curseforge_files.modId_1"));
    assert!(first.iter().any(|n| n == "modrinth_files._id.sha1_1"));
    assert!(first.iter().any(|n| n == "modrinth_files._id.sha512_1"));

    // 同步时按 project_id 删文件，复合索引的最左前缀就是 project_id
    assert!(
        first
            .iter()
            .any(|n| n == "modrinth_files.project_id_1_version_id_1_filename_1")
    );

    // 查「多久没核对过」要走索引，否则每次都是全表扫描
    assert!(first.iter().any(|n| n == "curseforge_mods.checked_at_1"));
    assert!(first.iter().any(|n| n == "modrinth_projects.checked_at_1"));

    let names: Vec<String> = db
        .collection::<Document>("modrinth_files")
        .list_index_names()
        .await
        .unwrap();
    assert!(names.contains(&"_id.sha1_1".to_string()));
}

/// 删除项目要连翻译记录一起清掉，否则 mcim-translate 会一直处理已经不存在的项目
#[tokio::test]
async fn removing_a_project_clears_its_translation() {
    let Some(db) = database("mcim_test_remove").await else {
        return;
    };
    for name in [
        collection::MODRINTH_PROJECTS,
        collection::MODRINTH_VERSIONS,
        collection::MODRINTH_FILES,
        collection::MODRINTH_TRANSLATED,
    ] {
        db.delete_many(name, doc! {}).await.unwrap();
    }

    let projects: Vec<mr::Project> = load("db_modrinth_projects.json");
    let versions: Vec<mr::Version> = load("db_modrinth_versions.json");
    let files: Vec<mr::File> = load("db_modrinth_files.json");
    let target = projects[0].id.clone();

    db.upsert_many(collection::MODRINTH_PROJECTS, &projects, 4).await.unwrap();
    db.upsert_many(collection::MODRINTH_VERSIONS, &versions, 4).await.unwrap();
    db.upsert_many(collection::MODRINTH_FILES, &files, 4).await.unwrap();
    let translation = ModrinthTranslation {
        project_id: target.clone(),
        translated: Some("译文".to_string()),
        original: Some("original".to_string()),
        need_to_update: false,
        translated_at: None,
    };
    db.upsert_many(collection::MODRINTH_TRANSLATED, &[translation], 1).await.unwrap();
    assert_eq!(db.count(collection::MODRINTH_TRANSLATED).await.unwrap(), 1);

    // 直接走删除路径用到的那几条语句
    db.delete_many(collection::MODRINTH_FILES, doc! { "project_id": &target }).await.unwrap();
    db.delete_many(collection::MODRINTH_VERSIONS, doc! { "project_id": &target }).await.unwrap();
    db.delete_by_id(collection::MODRINTH_TRANSLATED, &target).await.unwrap();
    db.delete_by_id(collection::MODRINTH_PROJECTS, &target).await.unwrap();

    assert_eq!(db.count(collection::MODRINTH_TRANSLATED).await.unwrap(), 0, "翻译记录没被清掉");
    assert_eq!(
        db.collection::<Document>(collection::MODRINTH_PROJECTS)
            .count_documents(doc! { "_id": &target })
            .await
            .unwrap(),
        0
    );

    for name in [
        collection::MODRINTH_PROJECTS,
        collection::MODRINTH_VERSIONS,
        collection::MODRINTH_FILES,
        collection::MODRINTH_TRANSLATED,
    ] {
        db.delete_many(name, doc! {}).await.unwrap();
    }
}

/// `touch_checked` 写下的 checked_at 不能被后续的整文档替换抹掉
///
/// upsert_many 走的是 replace_one，模型里没有的字段会连带消失，
/// 所以模型必须自己带着这个字段，光靠 $set 补是不够的
#[tokio::test]
async fn checked_at_survives_a_full_document_replace() {
    let Some(db) = database("mcim_test_checked").await else {
        return;
    };
    let collection = collection::CURSEFORGE_MODS;
    db.delete_many(collection, doc! {}).await.unwrap();

    let mods: Vec<cf::Mod> = load("db_curseforge_mods.json");
    db.upsert_many(collection, &mods, 4).await.unwrap();

    let touched = db
        .touch_checked(collection, &[mods[0].id], Utc::now())
        .await
        .unwrap();
    assert_eq!(touched, 1);

    let stored = fetch(&db, collection, mods[0].id).await;
    assert!(
        matches!(stored.get("checked_at"), Some(bson::Bson::DateTime(_))),
        "touch_checked 没写进去"
    );

    // 再同步一次这条 mod，走的就是 sync_mod 的整文档替换
    let mut refreshed = mods[0].clone();
    refreshed.checked_at = Some(refreshed.sync_at);
    db.upsert_many(collection, &[refreshed], 1).await.unwrap();

    let stored = fetch(&db, collection, mods[0].id).await;
    assert!(
        matches!(stored.get("checked_at"), Some(bson::Bson::DateTime(_))),
        "整文档替换把 checked_at 抹掉了"
    );
}

async fn fetch(db: &Database, collection: &str, id: i32) -> Document {
    db.collection::<Document>(collection)
        .find_one(doc! { "_id": id })
        .await
        .unwrap()
        .expect("文档丢了")
}
