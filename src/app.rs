use std::sync::Arc;

use crate::api::{CurseForgeApi, ModrinthApi};
use crate::config::Config;
use crate::db::{Database, Queues};
use crate::error::Result;
use crate::http::HttpClient;
use crate::sync::curseforge::CurseForgeSync;
use crate::sync::modrinth::ModrinthSync;

/// 一次运行所需的全部依赖
pub struct App {
    pub config: Config,
    pub db: Database,
    pub queues: Queues,
    http: Arc<HttpClient>,
}

impl App {
    pub async fn new(config: Config) -> Result<Self> {
        // 配置可以完全来自环境变量，把最终连到哪儿打出来，
        // 免得配错了却一路跑到写库才发现
        tracing::info!(
            mongodb = format!(
                "{}:{}/{}",
                config.mongodb.host, config.mongodb.port, config.mongodb.database
            ),
            redis = format!(
                "{}:{}/{}",
                config.redis.host, config.redis.port, config.redis.database
            ),
            "连接目标"
        );

        let http = Arc::new(HttpClient::new(&config)?);
        let db = Database::connect(&config).await?;
        let queues = Queues::connect(&config).await?;
        Ok(Self {
            config,
            db,
            queues,
            http,
        })
    }

    /// CurseForge 需要 API Key，只在真正用到时才构造
    pub fn curseforge(&self) -> Result<CurseForgeSync> {
        let api = CurseForgeApi::new(Arc::clone(&self.http), &self.config)?;
        Ok(CurseForgeSync::new(
            api,
            self.db.clone(),
            self.config.max_workers,
        ))
    }

    pub fn modrinth(&self) -> ModrinthSync {
        let api = ModrinthApi::new(Arc::clone(&self.http), &self.config);
        ModrinthSync::new(api, self.db.clone(), self.config.max_workers)
    }
}
