use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

fn default_max_workers() -> usize {
    8
}
fn default_curseforge_chunk_size() -> usize {
    1000
}
fn default_modrinth_chunk_size() -> usize {
    100
}
fn default_curseforge_api() -> String {
    "https://api.curseforge.com".to_string()
}
fn default_modrinth_api() -> String {
    "https://api.modrinth.com".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct MongodbConfig {
    pub host: String,
    pub port: u16,
    pub auth: bool,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: String,
}

impl Default for MongodbConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 27017,
            auth: false,
            user: None,
            password: None,
            database: "database".to_string(),
        }
    }
}

impl MongodbConfig {
    pub fn uri(&self) -> String {
        if self.auth {
            let user = self.user.as_deref().unwrap_or_default();
            let password = self.password.as_deref().unwrap_or_default();
            format!(
                "mongodb://{}:{}@{}:{}/?authSource=admin",
                urlencode(user),
                urlencode(password),
                self.host,
                self.port
            )
        } else {
            format!("mongodb://{}:{}", self.host, self.port)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RedisConfig {
    pub host: String,
    pub port: u16,
    pub password: Option<String>,
    pub database: u8,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            host: "localhost".to_string(),
            port: 6379,
            password: None,
            database: 0,
        }
    }
}

impl RedisConfig {
    pub fn uri(&self) -> String {
        match self.password.as_deref() {
            Some(password) if !password.is_empty() => format!(
                "redis://:{}@{}:{}/{}",
                urlencode(password),
                self.host,
                self.port,
                self.database
            ),
            _ => format!("redis://{}:{}/{}", self.host, self.port, self.database),
        }
    }
}

/// 令牌桶参数，按域名生效
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct RateLimit {
    pub capacity: u32,
    pub refill_rate: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub debug: bool,

    #[serde(default)]
    pub mongodb: MongodbConfig,

    #[serde(default)]
    pub redis: RedisConfig,

    /// 单个任务内的最大并发请求数
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,

    /// 批量接口一次提交的 id 数量
    #[serde(default = "default_curseforge_chunk_size")]
    pub curseforge_chunk_size: usize,
    #[serde(default = "default_modrinth_chunk_size")]
    pub modrinth_chunk_size: usize,

    #[serde(default = "default_curseforge_api")]
    pub curseforge_api: String,
    #[serde(default = "default_modrinth_api")]
    pub modrinth_api: String,

    #[serde(default)]
    pub curseforge_api_key: String,

    #[serde(default, alias = "proxies")]
    pub proxy: Option<String>,

    #[serde(default)]
    pub domain_rate_limits: HashMap<String, RateLimit>,
}

impl Config {
    /// 从 config.json 读取，缺失的键回落到默认值
    ///
    /// 与 Python 版不同，文件不存在时直接报错而不是写出一份默认配置
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            Error::Config(format!("读取 {} 失败: {}", path.display(), e))
        })?;
        let mut config: Config = serde_json::from_str(&raw)
            .map_err(|e| Error::Config(format!("解析 {} 失败: {}", path.display(), e)))?;
        config.apply_env();
        Ok(config)
    }

    /// 允许用环境变量覆盖密钥，避免明文落在配置文件里
    fn apply_env(&mut self) {
        if let Ok(key) = std::env::var("MCIM_CURSEFORGE_API_KEY") {
            self.curseforge_api_key = key;
        }
        if let Ok(password) = std::env::var("MCIM_MONGODB_PASSWORD") {
            self.mongodb.password = Some(password);
        }
        if let Ok(password) = std::env::var("MCIM_REDIS_PASSWORD") {
            self.redis.password = Some(password);
        }
    }
}

fn urlencode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}
