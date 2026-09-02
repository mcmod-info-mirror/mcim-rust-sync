use std::collections::{BTreeMap, HashMap};
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
fn default_shutdown_grace_secs() -> u64 {
    60
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

/// 守护模式下的一条定时计划
#[derive(Debug, Clone, Deserialize)]
pub struct ScheduleEntry {
    /// cron 表达式，按 UTC 判定
    pub cron: String,

    /// 要跑的子命令，写法与命令行一致
    pub args: String,
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

    /// 守护模式的定时计划，键是任务名，只用于日志；缺省表示不排期
    #[serde(default)]
    pub schedule: BTreeMap<String, ScheduleEntry>,

    /// 收到停止信号后留给在跑任务收尾的秒数
    #[serde(default = "default_shutdown_grace_secs")]
    pub shutdown_grace_secs: u64,
}

/// 环境变量来源，测试时可以换成别的
struct Env<F>(F);

impl<F: Fn(&str) -> Option<String>> Env<F> {
    /// 空字符串按未设置处理，免得空的环境变量把配置里的值清掉
    fn text(&self, key: &str) -> Option<String> {
        (self.0)(key).filter(|value| !value.is_empty())
    }

    fn parse<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>> {
        match self.text(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<T>()
                .map(Some)
                .map_err(|_| Error::Config(format!("{} 的值 {:?} 解析不了", key, raw))),
        }
    }

    fn json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        match self.text(key) {
            None => Ok(None),
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| Error::Config(format!("{} 不是合法 JSON: {}", key, e))),
        }
    }
}

impl Config {
    /// 读取配置，文件不存在时全部走默认值加环境变量
    ///
    /// 容器里通常只给环境变量、不挂配置文件。文件存在但解析不了仍然直接失败，
    /// 免得带着半份配置跑起来
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                tracing::info!(path = %path.display(), "已加载配置文件");
                Self::from_json(&raw)
                    .map_err(|e| Error::Config(format!("解析 {} 失败: {}", path.display(), e)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::info!(path = %path.display(), "配置文件不存在，只用默认值与环境变量");
                Self::from_json("{}")
            }
            Err(e) => Err(Error::Config(format!(
                "读取 {} 失败: {}",
                path.display(),
                e
            ))),
        }
    }

    /// 从 JSON 文本构造，缺失的键回落到默认值
    pub fn from_json(raw: &str) -> Result<Self> {
        let mut config: Config =
            serde_json::from_str(raw).map_err(|e| Error::Config(e.to_string()))?;
        config.apply_overrides(&Env(|key: &str| std::env::var(key).ok()))?;
        Ok(config)
    }

    /// 环境变量覆盖配置文件，容器里可以完全不挂文件
    ///
    /// schedule 与 domain_rate_limits 是映射，只能整体用 JSON 覆盖
    fn apply_overrides<F: Fn(&str) -> Option<String>>(&mut self, env: &Env<F>) -> Result<()> {
        if let Some(value) = env.parse("MCIM_DEBUG")? {
            self.debug = value;
        }

        if let Some(value) = env.text("MCIM_MONGODB_HOST") {
            self.mongodb.host = value;
        }
        if let Some(value) = env.parse("MCIM_MONGODB_PORT")? {
            self.mongodb.port = value;
        }
        if let Some(value) = env.parse("MCIM_MONGODB_AUTH")? {
            self.mongodb.auth = value;
        }
        if let Some(value) = env.text("MCIM_MONGODB_USER") {
            self.mongodb.user = Some(value);
        }
        if let Some(value) = env.text("MCIM_MONGODB_PASSWORD") {
            self.mongodb.password = Some(value);
        }
        if let Some(value) = env.text("MCIM_MONGODB_DATABASE") {
            self.mongodb.database = value;
        }

        if let Some(value) = env.text("MCIM_REDIS_HOST") {
            self.redis.host = value;
        }
        if let Some(value) = env.parse("MCIM_REDIS_PORT")? {
            self.redis.port = value;
        }
        if let Some(value) = env.text("MCIM_REDIS_PASSWORD") {
            self.redis.password = Some(value);
        }
        if let Some(value) = env.parse("MCIM_REDIS_DATABASE")? {
            self.redis.database = value;
        }

        if let Some(value) = env.parse("MCIM_MAX_WORKERS")? {
            self.max_workers = value;
        }
        if let Some(value) = env.parse("MCIM_CURSEFORGE_CHUNK_SIZE")? {
            self.curseforge_chunk_size = value;
        }
        if let Some(value) = env.parse("MCIM_MODRINTH_CHUNK_SIZE")? {
            self.modrinth_chunk_size = value;
        }

        if let Some(value) = env.text("MCIM_CURSEFORGE_API") {
            self.curseforge_api = value;
        }
        if let Some(value) = env.text("MCIM_MODRINTH_API") {
            self.modrinth_api = value;
        }
        if let Some(value) = env.text("MCIM_CURSEFORGE_API_KEY") {
            self.curseforge_api_key = value;
        }
        if let Some(value) = env.text("MCIM_PROXY") {
            self.proxy = Some(value);
        }

        if let Some(value) = env.json("MCIM_DOMAIN_RATE_LIMITS")? {
            self.domain_rate_limits = value;
        }
        if let Some(value) = env.json("MCIM_SCHEDULE")? {
            self.schedule = value;
        }
        if let Some(value) = env.parse("MCIM_SHUTDOWN_GRACE_SECS")? {
            self.shutdown_grace_secs = value;
        }

        Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// 只解析 JSON，不应用真实环境变量
    ///
    /// `Config::from_json` 会读进程 env，拿它做基线的话
    /// 测试结果取决于跑测试的机器上设了什么
    fn parse_only(raw: &str) -> Config {
        serde_json::from_str(raw).expect("解析失败")
    }

    /// 用假的环境变量来源，不碰进程全局 env，测试之间不会互相干扰
    fn with_env(pairs: &[(&str, &str)]) -> Result<Config> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let mut config: Config = serde_json::from_str("{}").expect("默认配置构造失败");
        config.apply_overrides(&Env(|key: &str| map.get(key).cloned()))?;
        Ok(config)
    }

    /// 容器里不挂配置文件，全部字段都要能从环境变量给
    #[test]
    fn env_alone_is_enough() {
        let config = with_env(&[
            ("MCIM_MONGODB_HOST", "mongo"),
            ("MCIM_MONGODB_PORT", "27018"),
            ("MCIM_MONGODB_DATABASE", "mcim"),
            ("MCIM_REDIS_HOST", "redis"),
            ("MCIM_REDIS_DATABASE", "3"),
            ("MCIM_MAX_WORKERS", "16"),
            ("MCIM_CURSEFORGE_API_KEY", "key"),
            ("MCIM_SHUTDOWN_GRACE_SECS", "90"),
        ])
        .expect("解析失败");

        assert_eq!(config.mongodb.host, "mongo");
        assert_eq!(config.mongodb.port, 27018);
        assert_eq!(config.mongodb.database, "mcim");
        assert_eq!(config.redis.host, "redis");
        assert_eq!(config.redis.database, 3);
        assert_eq!(config.max_workers, 16);
        assert_eq!(config.curseforge_api_key, "key");
        assert_eq!(config.shutdown_grace_secs, 90);
    }

    /// 映射类型只能整体用 JSON 覆盖
    #[test]
    fn maps_come_from_json() {
        let config = with_env(&[
            (
                "MCIM_SCHEDULE",
                r#"{"modrinth-tags":{"cron":"0 0 * * *","args":"modrinth tags"}}"#,
            ),
            (
                "MCIM_DOMAIN_RATE_LIMITS",
                r#"{"api.modrinth.com":{"capacity":100,"refill_rate":3}}"#,
            ),
        ])
        .expect("解析失败");

        assert_eq!(config.schedule.len(), 1);
        assert_eq!(config.schedule["modrinth-tags"].cron, "0 0 * * *");
        assert_eq!(config.domain_rate_limits["api.modrinth.com"].refill_rate, 3);
    }

    /// 没设置的键不能动配置文件里的值
    #[test]
    fn absent_keys_leave_the_file_alone() {
        let mut config = parse_only(r#"{"max_workers": 4}"#);
        config
            .apply_overrides(&Env(|_: &str| None))
            .expect("覆盖失败");
        assert_eq!(config.max_workers, 4);
    }

    /// 空字符串按未设置处理，否则 compose 里留个空值就把密钥清掉了
    #[test]
    fn empty_value_is_not_an_override() {
        let mut config = parse_only(r#"{"curseforge_api_key": "real"}"#);
        let map: HashMap<String, String> =
            [("MCIM_CURSEFORGE_API_KEY".to_string(), String::new())].into();
        config
            .apply_overrides(&Env(|key: &str| map.get(key).cloned()))
            .expect("覆盖失败");
        assert_eq!(config.curseforge_api_key, "real");
    }

    /// 值写错要当场报出来，不能默默忽略后按默认值跑
    #[test]
    fn unparsable_value_is_an_error() {
        assert!(with_env(&[("MCIM_MAX_WORKERS", "很多")]).is_err());
        assert!(with_env(&[("MCIM_SCHEDULE", "not json")]).is_err());
    }
}
