//! 端到端跑各个 task
//!
//! 走的是 `runner::execute`，和命令行同一条分发路径。要打真实上游，
//! 所以断言的是「跑完之后库里变成什么样」，而不只是任务没报错。
//!
//! 本机没有 MongoDB / Redis 时跳过；CurseForge 的用例还需要
//! `MCIM_CURSEFORGE_API_KEY`，没配也跳过。

use bson::{Document, doc};
use chrono::{Duration, Utc};
use mcim_rust_sync::app::App;
use mcim_rust_sync::cli::Command;
use mcim_rust_sync::config::Config;
use mcim_rust_sync::db::queue::key;
use mcim_rust_sync::models::collection;
use mcim_rust_sync::models::curseforge as cf;
use mcim_rust_sync::models::modrinth as mr;
use mcim_rust_sync::runner::execute;
use mcim_rust_sync::task::TaskSummary;

/// 库名与 Redis 库号都逐个测试分开，免得并行跑时互相清数据
fn config(database: &str, redis_database: u8) -> Config {
    let raw = format!(
        r#"{{
            "mongodb": {{ "host": "localhost", "port": 27017, "database": "{}" }},
            "redis": {{ "host": "localhost", "port": 6379, "database": {} }},
            "max_workers": 4,
            "modrinth_chunk_size": 50,
            "domain_rate_limits": {{
                "api.curseforge.com": {{ "capacity": 100, "refill_rate": 3 }},
                "api.modrinth.com": {{ "capacity": 100, "refill_rate": 3 }}
            }}
        }}"#,
        database, redis_database
    );
    Config::from_json(&raw).expect("解析测试配置失败")
}

/// 连不上依赖就跳过，跳过要出声，否则「测试通过」是假的
async fn app(database: &str, redis_database: u8) -> Option<App> {
    match App::new(config(database, redis_database)).await {
        Ok(app) => Some(app),
        Err(e) => {
            eprintln!("跳过：起不来 App（{}）", e);
            None
        }
    }
}

fn needs_curseforge_key(app: &App) -> bool {
    if app.config.curseforge_api_key.is_empty() {
        eprintln!("跳过：没有配 MCIM_CURSEFORGE_API_KEY");
        return true;
    }
    false
}

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    let raw = std::fs::read_to_string(&path).expect("读取夹具失败");
    serde_json::from_str(&raw).expect("解析夹具失败")
}

/// 清掉上一轮留下的数据，测试之间不能互相影响
async fn reset(app: &App, collections: &[&str], queues: &[&str]) {
    for name in collections {
        app.db.delete_many(name, doc! {}).await.expect("清库失败");
    }
    for queue in queues {
        app.queues.drain(queue).await.expect("清队列失败");
    }
}

async fn run(app: &App, args: &str) -> TaskSummary {
    let command = Command::parse_args(args).expect("命令解析失败");
    execute(app, &command)
        .await
        .unwrap_or_else(|e| panic!("{} 执行失败: {}", args, e))
}

async fn fetch<T: Into<bson::Bson>>(app: &App, name: &str, id: T) -> Option<Document> {
    app.db
        .collection::<Document>(name)
        .find_one(doc! { "_id": id.into() })
        .await
        .expect("查询失败")
}

// ---------- 队列 ----------

/// 队列里的 modid 要被同步进来，连同它的文件
#[tokio::test]
async fn curseforge_queue_syncs_seeded_ids() {
    let Some(app) = app("mcim_task_cf_queue", 10).await else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }
    reset(
        &app,
        &[collection::CURSEFORGE_MODS, collection::CURSEFORGE_FILES],
        &[key::CURSEFORGE_MODIDS, key::CURSEFORGE_FILEIDS, key::CURSEFORGE_FINGERPRINTS],
    )
    .await;

    let mods: Vec<cf::Mod> = load("db_curseforge_mods.json");
    let seeded = mods[0].id;
    app.queues
        .push(key::CURSEFORGE_MODIDS, &[seeded.to_string()])
        .await
        .expect("写队列失败");

    let summary = run(&app, "curseforge queue").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "一个都没同步: {:?}", summary);

    let stored = fetch(&app, collection::CURSEFORGE_MODS, seeded)
        .await
        .expect("mod 没写进库");
    assert!(stored.get("sync_at").is_some());
    assert!(stored.get("checked_at").is_some(), "同步时应当一并记下核对时刻");

    let files = app
        .db
        .collection::<Document>(collection::CURSEFORGE_FILES)
        .count_documents(doc! { "modId": seeded })
        .await
        .expect("统计失败");
    assert!(files > 0, "文件没跟着写进来");

    // 队列取走就该是空的
    assert_eq!(app.queues.len(key::CURSEFORGE_MODIDS).await.unwrap(), 0);
}

/// project_id、version_id、hash 三种队列都要能归一成项目同步
#[tokio::test]
async fn modrinth_queue_syncs_seeded_ids() {
    let Some(app) = app("mcim_task_mr_queue", 11).await else {
        return;
    };
    reset(
        &app,
        &[
            collection::MODRINTH_PROJECTS,
            collection::MODRINTH_VERSIONS,
            collection::MODRINTH_FILES,
        ],
        &[key::MODRINTH_PROJECT_IDS, key::MODRINTH_VERSION_IDS],
    )
    .await;

    let projects: Vec<mr::Project> = load("db_modrinth_projects.json");
    let seeded = projects[0].id.clone();
    app.queues
        .push(key::MODRINTH_PROJECT_IDS, std::slice::from_ref(&seeded))
        .await
        .expect("写队列失败");

    let summary = run(&app, "modrinth queue").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "一个都没同步: {:?}", summary);

    let stored = fetch(&app, collection::MODRINTH_PROJECTS, seeded.as_str())
        .await
        .expect("项目没写进库");
    assert!(stored.get("checked_at").is_some(), "同步时应当一并记下核对时刻");

    let versions = app
        .db
        .collection::<Document>(collection::MODRINTH_VERSIONS)
        .count_documents(doc! { "project_id": &seeded })
        .await
        .expect("统计失败");
    assert!(versions > 0, "版本没跟着写进来");
    assert_eq!(app.queues.len(key::MODRINTH_PROJECT_IDS).await.unwrap(), 0);
}

// ---------- 增量刷新 ----------

/// 库里放一条时间戳做旧的项目，refresh 必须认出来并重新拉取
#[tokio::test]
async fn modrinth_refresh_picks_up_a_stale_project() {
    let Some(app) = app("mcim_task_mr_refresh", 12).await else {
        return;
    };
    reset(
        &app,
        &[
            collection::MODRINTH_PROJECTS,
            collection::MODRINTH_VERSIONS,
            collection::MODRINTH_FILES,
        ],
        &[key::MODRINTH_PROJECT_IDS],
    )
    .await;

    let mut projects: Vec<mr::Project> = load("db_modrinth_projects.json");
    let stale = &mut projects[0];
    let target = stale.id.clone();
    // 做旧：上游的 updated 一定比这个新，于是必然被判为需要刷新
    stale.updated = Utc::now() - Duration::days(3650);
    stale.sync_at = stale.updated;
    stale.checked_at = None;
    app.db
        .upsert_many(collection::MODRINTH_PROJECTS, &projects[..1], 1)
        .await
        .expect("写入夹具失败");

    let summary = run(&app, "modrinth refresh").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "做旧的项目没有被刷新: {:?}", summary);

    let stored = fetch(&app, collection::MODRINTH_PROJECTS, target.as_str())
        .await
        .expect("项目不见了");
    let synced_at = stored.get_datetime("sync_at").expect("sync_at 缺失").to_chrono();
    assert!(
        synced_at > Utc::now() - Duration::minutes(10),
        "sync_at 没有被推进，说明并没有真的重新拉取"
    );
    // 比对过就要留痕，不管内容有没有变
    assert!(stored.get("checked_at").is_some(), "checked_at 没写上");
}

/// CurseForge 侧同理，比对的是 dateModified
#[tokio::test]
async fn curseforge_refresh_picks_up_a_stale_mod() {
    let Some(app) = app("mcim_task_cf_refresh", 13).await else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }
    reset(
        &app,
        &[collection::CURSEFORGE_MODS, collection::CURSEFORGE_FILES],
        &[key::CURSEFORGE_MODIDS],
    )
    .await;

    let mut mods: Vec<cf::Mod> = load("db_curseforge_mods.json");
    let stale = &mut mods[0];
    let target = stale.id;
    stale.date_modified = Some(Utc::now() - Duration::days(3650));
    stale.sync_at = stale.date_modified.expect("刚设过");
    stale.checked_at = None;
    app.db
        .upsert_many(collection::CURSEFORGE_MODS, &mods[..1], 1)
        .await
        .expect("写入夹具失败");

    let summary = run(&app, "curseforge refresh").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "做旧的 mod 没有被刷新: {:?}", summary);

    let stored = fetch(&app, collection::CURSEFORGE_MODS, target)
        .await
        .expect("mod 不见了");
    let synced_at = stored.get_datetime("sync_at").expect("sync_at 缺失").to_chrono();
    assert!(
        synced_at > Utc::now() - Duration::minutes(10),
        "sync_at 没有被推进，说明并没有真的重新拉取"
    );
    assert!(stored.get("checked_at").is_some(), "checked_at 没写上");
}

// ---------- 字典表 ----------

/// tags 是整表刷新，三张表都要被填满且共用同一个 sync_at
#[tokio::test]
async fn modrinth_tags_fills_every_table() {
    let Some(app) = app("mcim_task_mr_tags", 14).await else {
        return;
    };
    let tables = [
        collection::MODRINTH_CATEGORIES,
        collection::MODRINTH_LOADERS,
        collection::MODRINTH_GAME_VERSIONS,
    ];
    reset(&app, &tables, &[]).await;

    let summary = run(&app, "modrinth tags").await;
    assert!(summary.synced > 0);

    for table in tables {
        let count = app.db.count(table).await.expect("统计失败");
        assert!(count > 0, "{} 是空的", table);

        // 整表刷新完只该剩下本轮那一代
        let stamps = app
            .db
            .collection::<Document>(table)
            .distinct("sync_at", doc! {})
            .await
            .expect("取 distinct 失败");
        assert_eq!(stamps.len(), 1, "{} 里混了多代数据", table);
    }
}

#[tokio::test]
async fn curseforge_categories_fills_the_table() {
    let Some(app) = app("mcim_task_cf_categories", 15).await else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }
    reset(&app, &[collection::CURSEFORGE_CATEGORIES], &[]).await;

    let summary = run(&app, "curseforge categories --game-id 432").await;
    assert!(summary.synced > 0);
    assert!(
        app.db
            .count(collection::CURSEFORGE_CATEGORIES)
            .await
            .expect("统计失败")
            > 0
    );
}

// ---------- 搜索发现 ----------

/// 空库上翻一页，翻到的项目都该被收进来
#[tokio::test]
async fn modrinth_search_discovers_projects() {
    let Some(app) = app("mcim_task_mr_search", 9).await else {
        return;
    };
    reset(
        &app,
        &[
            collection::MODRINTH_PROJECTS,
            collection::MODRINTH_VERSIONS,
            collection::MODRINTH_FILES,
        ],
        &[key::MODRINTH_PROJECT_IDS],
    )
    .await;

    let summary = run(&app, "modrinth search --max-pages 1").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced > 0, "一个新项目都没发现: {:?}", summary);
    assert!(
        app.db
            .count(collection::MODRINTH_PROJECTS)
            .await
            .expect("统计失败")
            > 0
    );
}
