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
