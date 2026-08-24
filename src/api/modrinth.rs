use std::collections::HashMap;
use std::sync::Arc;

use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::Result;
use crate::http::HttpClient;
use crate::models::modrinth::{Category, GameVersion, Loader, Project, Version};

#[derive(Debug, Deserialize)]
pub struct SearchHit {
    pub project_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub offset: i64,
    pub limit: i64,
    pub total_hits: i64,
}

#[derive(Serialize)]
struct HashesBody<'a> {
    hashes: &'a [String],
    algorithm: &'a str,
}

pub struct ModrinthApi {
    http: Arc<HttpClient>,
    base: String,
    headers: HeaderMap,
}

impl ModrinthApi {
    pub fn new(http: Arc<HttpClient>, config: &Config) -> Self {
        Self {
            http,
            base: config.modrinth_api.trim_end_matches('/').to_string(),
            headers: HeaderMap::new(),
        }
    }

    pub async fn get_project(&self, project_id: &str) -> Result<Project> {
        let url = format!("{}/v2/project/{}", self.base, project_id);
        self.http.get(&url, &[], &self.headers).await
    }

    pub async fn get_project_versions(&self, project_id: &str) -> Result<Vec<Version>> {
        let url = format!("{}/v2/project/{}/version", self.base, project_id);
        self.http.get(&url, &[], &self.headers).await
    }

    pub async fn get_projects(&self, project_ids: &[String]) -> Result<Vec<Project>> {
        let url = format!("{}/v2/projects", self.base);
        let query = [("ids", serde_json::to_string(project_ids)?)];
        self.http.get(&url, &query, &self.headers).await
    }

    pub async fn get_versions(&self, version_ids: &[String]) -> Result<Vec<Version>> {
        let url = format!("{}/v2/versions", self.base);
        let query = [("ids", serde_json::to_string(version_ids)?)];
        self.http.get(&url, &query, &self.headers).await
    }

    /// hash 反查所在版本，algorithm 只支持 sha1 与 sha512
    pub async fn get_version_files(
        &self,
        hashes: &[String],
        algorithm: &str,
    ) -> Result<HashMap<String, Version>> {
        let url = format!("{}/v2/version_files", self.base);
        let body = HashesBody { hashes, algorithm };
        self.http.post(&url, &body, &self.headers).await
    }

    pub async fn get_categories(&self) -> Result<Vec<Category>> {
        let url = format!("{}/v2/tag/category", self.base);
        self.http.get(&url, &[], &self.headers).await
    }

    pub async fn get_loaders(&self) -> Result<Vec<Loader>> {
        let url = format!("{}/v2/tag/loader", self.base);
        self.http.get(&url, &[], &self.headers).await
    }

    pub async fn get_game_versions(&self) -> Result<Vec<GameVersion>> {
        let url = format!("{}/v2/tag/game_version", self.base);
        self.http.get(&url, &[], &self.headers).await
    }

    /// 按最新发布排序翻页，用于发现新收录的项目
    pub async fn search_newest(&self, offset: i64, limit: i64) -> Result<SearchResponse> {
        let url = format!("{}/v2/search", self.base);
        let query = [
            ("index", "newest".to_string()),
            ("offset", offset.to_string()),
            ("limit", limit.to_string()),
        ];
        self.http.get(&url, &query, &self.headers).await
    }
}
