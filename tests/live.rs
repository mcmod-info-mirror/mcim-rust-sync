//! 打真实上游的联网测试，默认跳过
//!
//! 用 `cargo test --test live -- --ignored` 手动执行

use std::sync::Arc;

use mcim_rust_sync::api::ModrinthApi;
use mcim_rust_sync::config::Config;
use mcim_rust_sync::http::HttpClient;

fn config() -> Config {
    let path = format!("{}/tests/fixtures/config.json", env!("CARGO_MANIFEST_DIR"));
    Config::load(std::path::Path::new(&path)).expect("加载配置失败")
}

fn api() -> ModrinthApi {
    let config = config();
    let http = Arc::new(HttpClient::new(&config).expect("构造 HTTP 客户端失败"));
    ModrinthApi::new(http, &config)
}

#[tokio::test]
#[ignore = "需要联网"]
async fn fetch_project_and_versions() {
    let api = api();

    let project = api.get_project("Wnxd13zP").await.expect("取项目失败");
    assert_eq!(project.slug, "clumps");
    assert!(!project.team.is_empty());

    let versions = api
        .get_project_versions("Wnxd13zP")
        .await
        .expect("取版本失败");
    assert!(!versions.is_empty());
    assert!(!versions[0].files.is_empty());
}

#[tokio::test]
#[ignore = "需要联网"]
async fn fetch_tags() {
    let api = api();
    assert!(!api.get_categories().await.expect("取分类失败").is_empty());
    assert!(!api.get_loaders().await.expect("取加载器失败").is_empty());
    assert!(
        !api.get_game_versions()
            .await
            .expect("取游戏版本失败")
            .is_empty()
    );
}

/// 限流生效时这几个连续请求会被拉开，但都应当成功
#[tokio::test]
#[ignore = "需要联网"]
async fn rate_limited_batch() {
    let api = api();
    let ids: Vec<String> = vec!["Wnxd13zP".into(), "P7dR8mSH".into(), "AANobbMI".into()];
    let projects = api.get_projects(&ids).await.expect("批量取项目失败");
    assert_eq!(projects.len(), 3);
}

/// 404 要能被识别成「不存在」而不是普通失败
#[tokio::test]
#[ignore = "需要联网"]
async fn missing_project_is_not_found() {
    let api = api();
    let error = api
        .get_project("definitely-not-a-real-project-id")
        .await
        .expect_err("应当返回错误");
    assert!(error.is_not_found(), "未识别为 404: {}", error);
    assert!(!error.is_retryable(), "404 不应当重试");
}

#[tokio::test]
#[ignore = "需要联网"]
async fn search_newest() {
    let api = api();
    let response = api.search_newest(0, 10).await.expect("搜索失败");
    assert_eq!(response.hits.len(), 10);
    assert!(response.total_hits > 0);
}
