//! 端到端跑各个 task
//!
//! 走的是 `runner::execute`，和命令行同一条分发路径。要打真实上游，
//! 所以断言的是「跑完之后库里变成什么样」，而不只是任务没报错。
//!
//! 没设 `DATABASE_URL`、连不上 PostgreSQL / Redis 时跳过；CurseForge 的用例还需要
//! `CURSEFORGE_API_KEY`，没配也跳过。

mod common;

use chrono::{DateTime, Duration, Utc};
use mcim_rust_sync::app::App;
use mcim_rust_sync::cli::Command;
use mcim_rust_sync::config::Config;
use mcim_rust_sync::db::queue::key;
use mcim_rust_sync::models::collection;
use mcim_rust_sync::models::curseforge as cf;
use mcim_rust_sync::models::modrinth as mr;
use mcim_rust_sync::runner::execute;
use mcim_rust_sync::task::TaskSummary;
use sqlx::AssertSqlSafe;

/// 库名与 Redis 库号都逐个测试分开，免得并行跑时互相清数据
fn config(database_url: String, redis_database: u8) -> Config {
    let raw = format!(
        r#"{{
            "redis": {{ "host": "localhost", "port": 6379, "database": {} }},
            "max_workers": 4,
            "modrinth_chunk_size": 50,
            "domain_rate_limits": {{
                "api.curseforge.com": {{ "capacity": 100, "refill_rate": 3 }},
                "api.modrinth.com": {{ "capacity": 100, "refill_rate": 3 }}
            }}
        }}"#,
        redis_database
    );
    let mut config = Config::from_json(&raw).expect("解析测试配置失败");
    // 环境变量里的 DATABASE_URL 指向的是共享实例，这里换成本测试独占的库
    config.database_url = database_url;
    config
}

/// 连不上依赖就跳过，跳过要出声，否则「测试通过」是假的
async fn app(database: &str, redis_database: u8, queues: &[&str]) -> Option<App> {
    let url = common::fresh_database(database).await?;
    let app = match App::new(config(url, redis_database)).await {
        Ok(app) => app,
        Err(e) => {
            eprintln!("跳过：起不来 App（{}）", e);
            return None;
        }
    };
    // 清掉上一轮留下的队列，测试之间不能互相影响
    for queue in queues {
        app.queues.drain(queue).await.expect("清队列失败");
    }
    Some(app)
}

fn needs_curseforge_key(app: &App) -> bool {
    if app.config.curseforge_api_key.is_empty() {
        eprintln!("跳过：没有配 CURSEFORGE_API_KEY");
        return true;
    }
    false
}

fn load<T: serde::de::DeserializeOwned>(name: &str) -> T {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    let raw = std::fs::read_to_string(&path).expect("读取夹具失败");
    serde_json::from_str(&raw).expect("解析夹具失败")
}

async fn run(app: &App, args: &str) -> TaskSummary {
    let command = Command::parse_args(args).expect("命令解析失败");
    execute(app, &command)
        .await
        .unwrap_or_else(|e| panic!("{} 执行失败: {}", args, e))
}

async fn count(app: &App, table: &str) -> i64 {
    sqlx::query_scalar(AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
        .fetch_one(app.db.pool())
        .await
        .expect("统计失败")
}

/// (sync_at, checked_at)
async fn mod_stamps(app: &App, id: i32) -> Option<(DateTime<Utc>, Option<DateTime<Utc>>)> {
    sqlx::query_as("SELECT sync_at, checked_at FROM curseforge_mods WHERE id = $1")
        .bind(id)
        .fetch_optional(app.db.pool())
        .await
        .expect("查询失败")
}

/// (sync_at, checked_at)
async fn project_stamps(app: &App, id: &str) -> Option<(DateTime<Utc>, Option<DateTime<Utc>>)> {
    sqlx::query_as("SELECT sync_at, checked_at FROM modrinth_projects WHERE id = $1")
        .bind(id)
        .fetch_optional(app.db.pool())
        .await
        .expect("查询失败")
}

// ---------- 队列 ----------

/// 队列里的 modid 要被同步进来，连同它的文件
#[tokio::test]
async fn curseforge_queue_syncs_seeded_ids() {
    let Some(app) = app(
        "mcim_task_cf_queue",
        10,
        &[
            key::CURSEFORGE_MODIDS,
            key::CURSEFORGE_FILEIDS,
            key::CURSEFORGE_FINGERPRINTS,
        ],
    )
    .await
    else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }

    let seeded = load::<cf::Mod>("api_curseforge_mod.json").id;
    app.queues
        .push(key::CURSEFORGE_MODIDS, &[seeded.to_string()])
        .await
        .expect("写队列失败");

    let summary = run(&app, "curseforge queue").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "一个都没同步: {:?}", summary);

    let (_, checked_at) = mod_stamps(&app, seeded).await.expect("mod 没写进库");
    assert!(checked_at.is_some(), "同步时应当一并记下核对时刻");

    let files: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM curseforge_files f \
         JOIN curseforge_file_records r ON r.record_id = f.record_id WHERE r.mod_id = $1",
    )
    .bind(seeded)
    .fetch_one(app.db.pool())
    .await
    .expect("统计失败");
    assert!(files > 0, "文件没跟着写进来");

    // 队列取走就该是空的
    assert_eq!(app.queues.len(key::CURSEFORGE_MODIDS).await.unwrap(), 0);
}

/// project_id、version_id、hash 三种队列都要能归一成项目同步
#[tokio::test]
async fn modrinth_queue_syncs_seeded_ids() {
    let Some(app) = app(
        "mcim_task_mr_queue",
        11,
        &[key::MODRINTH_PROJECT_IDS, key::MODRINTH_VERSION_IDS],
    )
    .await
    else {
        return;
    };

    let seeded = load::<mr::Project>("api_modrinth_project.json").id;
    app.queues
        .push(key::MODRINTH_PROJECT_IDS, std::slice::from_ref(&seeded))
        .await
        .expect("写队列失败");

    let summary = run(&app, "modrinth queue").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "一个都没同步: {:?}", summary);

    let (_, checked_at) = project_stamps(&app, &seeded).await.expect("项目没写进库");
    assert!(checked_at.is_some(), "同步时应当一并记下核对时刻");

    let versions: i64 =
        sqlx::query_scalar("SELECT count(*) FROM modrinth_versions WHERE project_id = $1")
            .bind(&seeded)
            .fetch_one(app.db.pool())
            .await
            .expect("统计失败");
    assert!(versions > 0, "版本没跟着写进来");
    assert_eq!(app.queues.len(key::MODRINTH_PROJECT_IDS).await.unwrap(), 0);
}

// ---------- 增量刷新 ----------

/// 库里放一条时间戳做旧的项目，refresh 必须认出来并重新拉取
#[tokio::test]
async fn modrinth_refresh_picks_up_a_stale_project() {
    let Some(app) = app("mcim_task_mr_refresh", 12, &[key::MODRINTH_PROJECT_IDS]).await else {
        return;
    };

    let mut stale: mr::Project = load("api_modrinth_project.json");
    let target = stale.id.clone();
    // 做旧：上游的 updated 一定比这个新，于是必然被判为需要刷新
    stale.updated = Utc::now() - Duration::days(3650);
    stale.sync_at = stale.updated;
    stale.checked_at = None;
    app.db
        .replace_modrinth_projects(&[stale])
        .await
        .expect("写入夹具失败");

    let summary = run(&app, "modrinth refresh").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "做旧的项目没有被刷新: {:?}", summary);

    let (synced_at, checked_at) = project_stamps(&app, &target).await.expect("项目不见了");
    assert!(
        synced_at > Utc::now() - Duration::minutes(10),
        "sync_at 没有被推进，说明并没有真的重新拉取"
    );
    // 比对过就要留痕，不管内容有没有变
    assert!(checked_at.is_some(), "checked_at 没写上");
}

/// CurseForge 侧同理，比对的是 dateModified
#[tokio::test]
async fn curseforge_refresh_picks_up_a_stale_mod() {
    let Some(app) = app("mcim_task_cf_refresh", 13, &[key::CURSEFORGE_MODIDS]).await else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }

    let mut stale: cf::Mod = load("api_curseforge_mod.json");
    let target = stale.id;
    stale.date_modified = Some(Utc::now() - Duration::days(3650));
    stale.sync_at = stale.date_modified.expect("刚设过");
    stale.checked_at = None;
    app.db
        .replace_curseforge_mods(&[stale])
        .await
        .expect("写入夹具失败");

    let summary = run(&app, "curseforge refresh").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced >= 1, "做旧的 mod 没有被刷新: {:?}", summary);

    let (synced_at, checked_at) = mod_stamps(&app, target).await.expect("mod 不见了");
    assert!(
        synced_at > Utc::now() - Duration::minutes(10),
        "sync_at 没有被推进，说明并没有真的重新拉取"
    );
    assert!(checked_at.is_some(), "checked_at 没写上");
}

// ---------- 字典表 ----------

/// tags 是整表刷新，三张表都要被填满且共用同一个 sync_at
#[tokio::test]
async fn modrinth_tags_fills_every_table() {
    let Some(app) = app("mcim_task_mr_tags", 14, &[]).await else {
        return;
    };

    // 连跑两轮，第二轮必须替换掉第一轮而不是叠加
    run(&app, "modrinth tags").await;
    let summary = run(&app, "modrinth tags").await;
    assert!(summary.synced > 0);

    for table in [
        collection::MODRINTH_CATEGORIES,
        collection::MODRINTH_LOADERS,
        collection::MODRINTH_GAME_VERSIONS,
    ] {
        assert!(count(&app, table).await > 0, "{} 是空的", table);

        // 整表刷新完只该剩下本轮那一代
        let generations: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT count(DISTINCT sync_at) FROM {table}"
        )))
        .fetch_one(app.db.pool())
        .await
        .expect("统计失败");
        assert_eq!(generations, 1, "{} 里混了多代数据", table);
    }
}

#[tokio::test]
async fn curseforge_categories_fills_the_table() {
    let Some(app) = app("mcim_task_cf_categories", 15, &[]).await else {
        return;
    };
    if needs_curseforge_key(&app) {
        return;
    }

    let summary = run(&app, "curseforge categories --game-id 432").await;
    assert!(summary.synced > 0);
    assert!(count(&app, collection::CURSEFORGE_CATEGORIES).await > 0);
}

// ---------- 搜索发现 ----------

/// 空库上翻一页，翻到的项目都该被收进来
#[tokio::test]
async fn modrinth_search_discovers_projects() {
    let Some(app) = app("mcim_task_mr_search", 9, &[key::MODRINTH_PROJECT_IDS]).await else {
        return;
    };

    let summary = run(&app, "modrinth search --max-pages 1").await;
    assert_eq!(summary.failed, 0, "有条目没同步成功");
    assert!(summary.synced > 0, "一个新项目都没发现: {:?}", summary);
    assert!(count(&app, collection::MODRINTH_PROJECTS).await > 0);
}

/// 分块遍历必须覆盖整张表，不能漏掉最后不满一块的尾巴
///
/// 库里塞 250 个上游不存在的项目、分块大小 50，也就是 5 块。
/// 上游对这些 id 全部返回空，所以「全部被判定为已删除」等价于
/// 「每一块都被处理到了」——漏掉任何一块都会有残留
#[tokio::test]
async fn refresh_walks_every_chunk() {
    let Some(app) = app("mcim_task_mr_chunks", 8, &[]).await else {
        return;
    };

    let template: mr::Project = load("api_modrinth_project.json");
    let seeded: Vec<mr::Project> = (0..250)
        .map(|i| {
            let mut project = template.clone();
            project.id = format!("zzTest{:04}", i);
            project
        })
        .collect();
    app.db
        .replace_modrinth_projects(&seeded)
        .await
        .expect("写入夹具失败");
    assert_eq!(count(&app, collection::MODRINTH_PROJECTS).await, 250);

    run(&app, "modrinth refresh").await;

    let left = count(&app, collection::MODRINTH_PROJECTS).await;
    assert_eq!(left, 0, "还剩 {} 个没被处理到，说明有块被漏掉", left);
}
