use redis::AsyncCommands;
use redis::aio::ConnectionManager;

use crate::config::Config;
use crate::error::Result;

/// mcim-rust-api 把未命中的请求参数写进这些 Set
pub mod key {
    pub const CURSEFORGE_MODIDS: &str = "curseforge_modids";
    pub const CURSEFORGE_FILEIDS: &str = "curseforge_fileids";
    pub const CURSEFORGE_FINGERPRINTS: &str = "curseforge_fingerprints";

    pub const MODRINTH_PROJECT_IDS: &str = "modrinth_project_ids";
    pub const MODRINTH_VERSION_IDS: &str = "modrinth_version_ids";

    /// Modrinth 的 `/v2/version_files` 只支持这两种算法
    pub const MODRINTH_HASH_ALGORITHMS: [&str; 2] = ["sha1", "sha512"];

    pub fn modrinth_hashes(algorithm: &str) -> String {
        format!("modrinth_hashes_{}", algorithm)
    }
}

/// 一次取走的成员数
const DRAIN_BATCH: usize = 1000;

#[derive(Clone)]
pub struct Queues {
    connection: ConnectionManager,
}

impl Queues {
    pub async fn connect(config: &Config) -> Result<Self> {
        let client = redis::Client::open(config.redis.uri())?;
        Ok(Self {
            connection: client.get_connection_manager().await?,
        })
    }

    /// 原子取走整个队列
    ///
    /// Python 版是 SMEMBERS 读完再 DELETE，两步之间 API 新写入的成员会被一并删掉。
    /// SPOP 取到的就是自己的，没取到的留在队列里等下一轮
    pub async fn drain(&self, key: &str) -> Result<Vec<String>> {
        let mut connection = self.connection.clone();
        let mut members = Vec::new();
        loop {
            let batch: Vec<String> = redis::cmd("SPOP")
                .arg(key)
                .arg(DRAIN_BATCH)
                .query_async(&mut connection)
                .await?;
            if batch.is_empty() {
                break;
            }
            let exhausted = batch.len() < DRAIN_BATCH;
            members.extend(batch);
            if exhausted {
                break;
            }
        }
        Ok(members)
    }

    /// 把没处理成功的成员放回队列，下一轮重试
    pub async fn push(&self, key: &str, members: &[String]) -> Result<()> {
        if members.is_empty() {
            return Ok(());
        }
        let mut connection = self.connection.clone();
        let _: usize = connection.sadd(key, members).await?;
        Ok(())
    }

    pub async fn len(&self, key: &str) -> Result<usize> {
        let mut connection = self.connection.clone();
        Ok(connection.scard(key).await?)
    }
}
