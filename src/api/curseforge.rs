use std::collections::HashMap;
use std::sync::Arc;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::http::HttpClient;
use crate::models::curseforge::{Category, File, Mod};

/// `/v1/mods/search` 的 sortField，11 是 ReleasedDate
pub const SORT_FIELD_RELEASED_DATE: i32 = 11;

#[derive(Debug, Deserialize)]
pub struct DataResponse<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct Pagination {
    pub index: i64,
    #[serde(rename = "pageSize")]
    pub page_size: i64,
    #[serde(rename = "resultCount")]
    pub result_count: i64,
    #[serde(rename = "totalCount")]
    pub total_count: i64,
}

#[derive(Debug, Deserialize)]
pub struct PaginatedResponse<T> {
    pub data: Vec<T>,
    pub pagination: Pagination,
}

/// `/v1/fingerprints` 的匹配结果，这里只关心它指向哪个 mod
#[derive(Debug, Deserialize)]
pub struct FingerprintMatch {
    pub id: i64,
    pub file: FingerprintFile,
}

#[derive(Debug, Deserialize)]
pub struct FingerprintFile {
    #[serde(rename = "modId")]
    pub mod_id: i32,
}

#[derive(Debug, Deserialize)]
pub struct FingerprintResult {
    #[serde(rename = "exactMatches", default)]
    pub exact_matches: Vec<FingerprintMatch>,
}

#[derive(Serialize)]
struct ModIdsBody<'a> {
    #[serde(rename = "modIds")]
    mod_ids: &'a [i32],
}

#[derive(Serialize)]
struct FileIdsBody<'a> {
    #[serde(rename = "fileIds")]
    file_ids: &'a [i32],
}

#[derive(Serialize)]
struct FingerprintsBody<'a> {
    fingerprints: &'a [i64],
}

/// 搜索的分片维度
///
/// 单个查询最多只能翻到一万条，靠分类切细才能覆盖全站
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchSlice {
    Class(i32),
    Category(i32),
}

impl SearchSlice {
    pub fn id(&self) -> i32 {
        match self {
            SearchSlice::Class(id) | SearchSlice::Category(id) => *id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            SearchSlice::Class(_) => "class",
            SearchSlice::Category(_) => "category",
        }
    }
}

pub struct CurseForgeApi {
    http: Arc<HttpClient>,
    base: String,
    headers: HeaderMap,
}

impl CurseForgeApi {
    pub fn new(http: Arc<HttpClient>, config: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if config.curseforge_api_key.is_empty() {
            return Err(Error::Config("curseforge_api_key 未配置".to_string()));
        }
        headers.insert(
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_str(&config.curseforge_api_key)
                .map_err(|e| Error::Config(format!("curseforge_api_key 不合法: {}", e)))?,
        );
        Ok(Self {
            http,
            base: config.curseforge_api.trim_end_matches('/').to_string(),
            headers,
        })
    }

    pub async fn get_mod(&self, mod_id: i32) -> Result<Mod> {
        let url = format!("{}/v1/mods/{}", self.base, mod_id);
        let response: DataResponse<Mod> = self.http.get(&url, &[], &self.headers).await?;
        Ok(response.data)
    }

    pub async fn get_mod_files(
        &self,
        mod_id: i32,
        index: i64,
        page_size: i64,
    ) -> Result<PaginatedResponse<File>> {
        let url = format!("{}/v1/mods/{}/files", self.base, mod_id);
        let query = [
            ("index", index.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        self.http.get(&url, &query, &self.headers).await
    }

    pub async fn get_mods(&self, mod_ids: &[i32]) -> Result<Vec<Mod>> {
        let url = format!("{}/v1/mods", self.base);
        let body = ModIdsBody { mod_ids };
        let response: DataResponse<Vec<Mod>> = self.http.post(&url, &body, &self.headers).await?;
        Ok(response.data)
    }

    pub async fn get_files(&self, file_ids: &[i32]) -> Result<Vec<File>> {
        let url = format!("{}/v1/mods/files", self.base);
        let body = FileIdsBody { file_ids };
        let response: DataResponse<Vec<File>> = self.http.post(&url, &body, &self.headers).await?;
        Ok(response.data)
    }

    pub async fn get_fingerprints(&self, fingerprints: &[i64]) -> Result<FingerprintResult> {
        let url = format!("{}/v1/fingerprints", self.base);
        let body = FingerprintsBody { fingerprints };
        let response: DataResponse<FingerprintResult> =
            self.http.post(&url, &body, &self.headers).await?;
        Ok(response.data)
    }

    pub async fn get_categories(
        &self,
        game_id: i32,
        class_id: Option<i32>,
        classes_only: bool,
    ) -> Result<Vec<Category>> {
        let url = format!("{}/v1/categories", self.base);
        let mut query = vec![("gameId", game_id.to_string())];
        if let Some(class_id) = class_id {
            query.push(("classId", class_id.to_string()));
        } else if classes_only {
            query.push(("classesOnly", "true".to_string()));
        }
        let response: DataResponse<Vec<Category>> =
            self.http.get(&url, &query, &self.headers).await?;
        Ok(response.data)
    }

    /// 按发布时间排序搜索
    pub async fn search(
        &self,
        game_id: i32,
        slice: SearchSlice,
        index: i64,
        page_size: i64,
        descending: bool,
    ) -> Result<PaginatedResponse<Mod>> {
        let url = format!("{}/v1/mods/search", self.base);
        let mut query = vec![
            ("gameId", game_id.to_string()),
            ("index", index.to_string()),
            ("pageSize", page_size.to_string()),
            ("sortField", SORT_FIELD_RELEASED_DATE.to_string()),
            (
                "sortOrder",
                if descending { "desc" } else { "asc" }.to_string(),
            ),
        ];
        match slice {
            SearchSlice::Class(id) => query.push(("classId", id.to_string())),
            SearchSlice::Category(id) => query.push(("categoryId", id.to_string())),
        }
        self.http.get(&url, &query, &self.headers).await
    }
}

/// fingerprint 结果里出现过的 modId
pub fn fingerprint_mod_ids(result: &FingerprintResult) -> Vec<i32> {
    let mut ids: Vec<i32> = result
        .exact_matches
        .iter()
        .map(|item| item.file.mod_id)
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// 供任务层记录未匹配上的指纹
pub fn matched_fingerprints(result: &FingerprintResult) -> HashMap<i64, i32> {
    result
        .exact_matches
        .iter()
        .map(|item| (item.id, item.file.mod_id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::SearchSlice;

    /// class 与 category 是搜索接口的两个不同参数，拿分类 id 去填 classId 会一条都查不到
    #[test]
    fn slice_carries_its_kind() {
        assert_eq!(SearchSlice::Class(6).kind(), "class");
        assert_eq!(SearchSlice::Category(424).kind(), "category");
        assert_eq!(SearchSlice::Class(6).id(), 6);
        assert_eq!(SearchSlice::Category(424).id(), 424);
        assert_ne!(SearchSlice::Class(424), SearchSlice::Category(424));
    }
}
