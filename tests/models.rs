//! 模型保真度测试
//!
//! `api_*` 夹具是上游响应，验证能直接吃下游 JSON 并补上 sync_at

use chrono::{DateTime, Utc};
use mcim_rust_sync::models::{curseforge as cf, modrinth as mr};

fn load(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {}", path, e))
}

fn parse<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&load(name)).unwrap_or_else(|e| panic!("解析 {} 失败: {}", name, e))
}

fn is_fresh(value: DateTime<Utc>) -> bool {
    (Utc::now() - value).num_seconds().abs() < 300
}

#[test]
fn api_modrinth_project() {
    let project: mr::Project = parse("api_modrinth_project.json");
    assert_eq!(project.id, "Wnxd13zP");
    assert_eq!(project.slug, "clumps");
    // 上游没有 sync_at，应当补成当前时间
    assert!(is_fresh(project.sync_at), "sync_at 未取到当前时间");
    // 上游没有 checked_at，同步时才写上
    assert!(project.checked_at.is_none());
}

#[test]
fn api_modrinth_versions() {
    let versions: Vec<mr::Version> = parse("api_modrinth_versions.json");
    assert_eq!(versions.len(), 2);
    assert!(is_fresh(versions[0].sync_at));
    assert!(!versions[0].files.is_empty());
    assert!(!versions[0].files[0].hashes.sha512.is_empty());
    assert!(!versions[0].files[0].hashes.sha1.is_empty());
}

#[test]
fn api_modrinth_tags() {
    let categories: Vec<mr::Category> = parse("api_modrinth_tag_category.json");
    let loaders: Vec<mr::Loader> = parse("api_modrinth_tag_loader.json");
    let game_versions: Vec<mr::GameVersion> = parse("api_modrinth_tag_game_version.json");
    assert_eq!(categories.len(), 3);
    assert_eq!(loaders.len(), 3);
    assert_eq!(game_versions.len(), 3);
    assert!(is_fresh(categories[0].sync_at));
}

#[test]
fn api_curseforge_mod() {
    let value: cf::Mod = parse("api_curseforge_mod.json");
    assert_eq!(value.id, 946010);
    assert_eq!(value.slug, "christmas-culinary");
    assert!(is_fresh(value.sync_at));
    assert!(value.date_modified.is_some());
    assert!(value.checked_at.is_none());
}
