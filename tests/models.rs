//! 模型保真度测试
//!
//! 两个方向都要验证：
//! - `db_*` 夹具来自 Python 版真实写出的库，验证新模型能读回既有数据
//! - `api_*` 夹具是上游响应，验证能直接吃下游 JSON 并补上 sync_at

use chrono::{DateTime, Utc};
use mcim_rust_sync::models::{curseforge as cf, modrinth as mr};

fn load(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读取 {} 失败: {}", path, e))
}

fn parse<T: serde::de::DeserializeOwned>(name: &str) -> T {
    serde_json::from_str(&load(name)).unwrap_or_else(|e| panic!("解析 {} 失败: {}", name, e))
}

/// 写回 MongoDB 时的实际形态
fn to_doc<T: serde::Serialize>(value: &T) -> bson::Document {
    bson::serialize_to_document(value).expect("序列化为 BSON 文档失败")
}

// ---------- 库内既有数据方向 ----------

#[test]
fn db_curseforge_mods() {
    let mods: Vec<cf::Mod> = parse("db_curseforge_mods.json");
    assert_eq!(mods.len(), 2);
    let first = &mods[0];
    assert_eq!(first.id, 946010);
    assert!(first.date_modified.is_some());
    assert!(first.latest_files.as_ref().is_some_and(|f| !f.is_empty()));

    // 嵌套 categories 里的 dateModified 在库里是 BSON 字符串，必须也能读出来
    let categories = first.categories.as_ref().expect("categories 缺失");
    assert!(categories[0].date_modified.is_some());

    let doc = to_doc(first);
    assert_eq!(doc.get_i32("_id").unwrap(), 946010);
    assert!(matches!(doc.get("sync_at"), Some(bson::Bson::DateTime(_))));
    // 归一化：写回时统一成 BSON DateTime，与 mcim-rust-api 的类型声明一致
    let nested = doc.get_array("categories").unwrap()[0].as_document().unwrap();
    assert!(matches!(nested.get("dateModified"), Some(bson::Bson::DateTime(_))));
}

#[test]
fn db_curseforge_files() {
    let files: Vec<cf::File> = parse("db_curseforge_files.json");
    assert_eq!(files.len(), 3);
    let doc = to_doc(&files[0]);
    assert!(doc.get_i32("_id").is_ok());
    assert!(matches!(doc.get("fileDate"), Some(bson::Bson::DateTime(_))));
    // 遗留字段不再写回
    assert!(!doc.contains_key("file_cdn_cached"));
    assert!(!doc.contains_key("found"));
}

#[test]
fn db_curseforge_categories() {
    let categories: Vec<cf::Category> = parse("db_curseforge_categories.json");
    assert_eq!(categories.len(), 4);
    let doc = to_doc(&categories[0]);
    assert!(doc.get_i32("_id").is_ok());
    assert!(matches!(doc.get("dateModified"), Some(bson::Bson::DateTime(_))));
    assert!(doc.get_i32("displayIndex").is_ok());
}

#[test]
fn db_modrinth_projects() {
    let projects: Vec<mr::Project> = parse("db_modrinth_projects.json");
    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].id, "Wnxd13zP");
    let doc = to_doc(&projects[0]);
    assert_eq!(doc.get_str("_id").unwrap(), "Wnxd13zP");
    assert!(matches!(doc.get("published"), Some(bson::Bson::DateTime(_))));
}

#[test]
fn db_modrinth_versions() {
    let versions: Vec<mr::Version> = parse("db_modrinth_versions.json");
    assert_eq!(versions.len(), 3);
    let doc = to_doc(&versions[0]);
    assert!(doc.get_str("_id").is_ok());
    assert!(!doc.get_array("files").unwrap().is_empty());
}

/// `modrinth_files` 的主键是子文档，字段顺序必须是 sha512 在前
///
/// MongoDB 判定子文档相等是顺序敏感的，换序会让同一个文件变成两条记录
#[test]
fn db_modrinth_files_id_field_order() {
    let files: Vec<mr::File> = parse("db_modrinth_files.json");
    assert_eq!(files.len(), 3);

    let doc = to_doc(&files[0]);
    let id = doc.get_document("_id").expect("_id 必须是子文档");
    let keys: Vec<&str> = id.keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["sha512", "sha1"], "_id 字段顺序被改变");

    assert!(!doc.contains_key("file_cdn_cached"));
    assert!(!doc.contains_key("found"));
}

#[test]
fn db_modrinth_tags() {
    let categories: Vec<mr::Category> = parse("db_modrinth_categories.json");
    let loaders: Vec<mr::Loader> = parse("db_modrinth_loaders.json");
    let game_versions: Vec<mr::GameVersion> = parse("db_modrinth_game_versions.json");
    assert_eq!(categories.len(), 4);
    assert_eq!(loaders.len(), 3);
    assert_eq!(game_versions.len(), 3);
}

// ---------- 上游响应方向 ----------

fn is_fresh(value: DateTime<Utc>) -> bool {
    (Utc::now() - value).num_seconds().abs() < 300
}

#[test]
fn api_modrinth_project() {
    let project: mr::Project = parse("api_modrinth_project.json");
    assert_eq!(project.slug, "clumps");
    // 上游没有 sync_at，应当补成当前时间
    assert!(is_fresh(project.sync_at), "sync_at 未取到当前时间");

    let doc = to_doc(&project);
    assert!(matches!(doc.get("sync_at"), Some(bson::Bson::DateTime(_))));
    assert!(matches!(doc.get("updated"), Some(bson::Bson::DateTime(_))));
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
    assert_eq!(value.slug, "christmas-culinary");
    assert!(is_fresh(value.sync_at));
    assert!(value.date_modified.is_some());

    let doc = to_doc(&value);
    assert!(doc.get_i32("_id").is_ok());
    assert!(matches!(doc.get("dateModified"), Some(bson::Bson::DateTime(_))));
}
