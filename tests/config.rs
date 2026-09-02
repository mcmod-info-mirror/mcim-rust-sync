//! 现有的 config.json 要能直接拿来用
//!
//! 已经废弃的键（job_config、interval、cron_trigger、telegram 等）会被忽略而不是报错

use std::path::Path;

use mcim_rust_sync::config::Config;

fn fixture() -> Config {
    let path = format!("{}/tests/fixtures/config.json", env!("CARGO_MANIFEST_DIR"));
    Config::load(Path::new(&path)).expect("加载配置失败")
}

#[test]
fn loads_existing_config() {
    let config = fixture();
    assert_eq!(config.max_workers, 4);
    assert_eq!(config.curseforge_chunk_size, 1000);
    assert_eq!(config.modrinth_chunk_size, 50);
    assert_eq!(config.mongodb.database, "mcim_backend");
    assert_eq!(config.redis.database, 0);
    assert_eq!(config.domain_rate_limits.len(), 4);
    assert_eq!(config.domain_rate_limits["api.curseforge.com"].refill_rate, 3);
}

#[test]
fn falls_back_to_official_api() {
    let config = fixture();
    assert_eq!(config.curseforge_api, "https://api.curseforge.com");
    assert_eq!(config.modrinth_api, "https://api.modrinth.com");
}

/// 密码里的特殊字符必须转义，否则连接串会被解析错
#[test]
fn escapes_mongodb_credentials() {
    let config = fixture();
    let uri = config.mongodb.uri();
    assert!(uri.contains("p%40ss%20word%2F1"), "密码未正确转义: {}", uri);
    assert!(uri.starts_with("mongodb://z0z0r4:"));
}

#[test]
fn redis_uri_without_password() {
    let config = fixture();
    assert_eq!(config.redis.uri(), "redis://localhost:6379/0");
}

#[test]
fn missing_file_is_an_error() {
    // Python 版在配置缺失时会写出一份默认配置，这里要直接失败
    let result = Config::load(Path::new("/nonexistent/config.json"));
    assert!(result.is_err());
}

/// 老配置里没有 schedule，守护模式之外的用法不受影响
#[test]
fn schedule_is_optional() {
    let config = fixture();
    assert!(config.schedule.is_empty());
    assert_eq!(config.shutdown_grace_secs, 60);
}

#[test]
fn loads_schedule() {
    let raw = r#"{
        "schedule": {
            "curseforge-queue": { "cron": "*/20 * * * *", "args": "curseforge queue" },
            "modrinth-tags": { "cron": "0 0 * * *", "args": "modrinth tags" }
        },
        "shutdown_grace_secs": 120
    }"#;
    let config = Config::from_json(raw).expect("解析配置失败");

    assert_eq!(config.schedule.len(), 2);
    assert_eq!(config.schedule["curseforge-queue"].cron, "*/20 * * * *");
    assert_eq!(config.schedule["modrinth-tags"].args, "modrinth tags");
    assert_eq!(config.shutdown_grace_secs, 120);
}
