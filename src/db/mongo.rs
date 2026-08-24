use bson::{Document, doc};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt, TryStreamExt};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::Config;
use crate::error::{Error, Result};

#[derive(Clone)]
pub struct Database {
    inner: mongodb::Database,
}

impl Database {
    pub async fn connect(config: &Config) -> Result<Self> {
        let mut options = ClientOptions::parse(config.mongodb.uri()).await?;
        options.app_name = Some("mcim-rust-sync".to_string());
        let client = Client::with_options(options)?;

        // 连不上就直接失败，不要等到写库时才发现
        client
            .database("admin")
            .run_command(doc! { "ping": 1 })
            .await?;

        Ok(Self {
            inner: client.database(&config.mongodb.database),
        })
    }

    pub fn collection<T: Send + Sync>(&self, name: &str) -> Collection<T> {
        self.inner.collection(name)
    }

    /// 按 `_id` 整文档替换写入
    ///
    /// 沿用 Python 版的替换语义，因此上游已删除的字段会一并消失
    pub async fn upsert_many<T>(&self, name: &str, items: &[T], concurrency: usize) -> Result<u64>
    where
        T: Serialize,
    {
        if items.is_empty() {
            return Ok(0);
        }

        let documents = items
            .iter()
            .map(|item| bson::serialize_to_document(item).map_err(Error::from))
            .collect::<Result<Vec<Document>>>()?;

        let collection = self.collection::<Document>(name);
        stream::iter(documents)
            .map(|document| {
                let collection = collection.clone();
                async move {
                    let id = document.get("_id").cloned().ok_or_else(|| {
                        Error::Config(format!("{} 的文档缺少 _id", collection.name()))
                    })?;
                    collection
                        .replace_one(doc! { "_id": id }, document)
                        .upsert(true)
                        .await?;
                    Ok::<(), Error>(())
                }
            })
            .buffer_unordered(concurrency.max(1))
            .try_fold(0u64, |count, ()| async move { Ok(count + 1) })
            .await
    }

    /// 整表刷新无主键的字典表
    ///
    /// 先写入本轮数据再删除上一轮，读方不会看到空集合。
    /// Python 版是先删后插，中途失败会把表清空
    pub async fn refresh_collection<T>(
        &self,
        name: &str,
        items: &[T],
        stamp: DateTime<Utc>,
    ) -> Result<u64>
    where
        T: Serialize,
    {
        if items.is_empty() {
            return Err(Error::Config(format!("{} 的新数据为空，拒绝刷新", name)));
        }

        let documents = items
            .iter()
            .map(|item| bson::serialize_to_document(item).map_err(Error::from))
            .collect::<Result<Vec<Document>>>()?;

        let collection = self.collection::<Document>(name);
        collection.insert_many(&documents).await?;
        collection
            .delete_many(doc! { "sync_at": { "$lt": bson::DateTime::from_chrono(stamp) } })
            .await?;

        Ok(documents.len() as u64)
    }

    /// 流式遍历整个集合，只取需要的字段
    ///
    /// 取代 Python 版无排序的 skip/limit 分页，后者在并发写入时会漏读或重读
    pub async fn stream_all<T>(&self, name: &str, projection: Document) -> Result<Vec<T>>
    where
        T: DeserializeOwned + Send + Sync,
    {
        let collection = self.collection::<T>(name);
        let cursor = collection
            .find(doc! {})
            .projection(projection)
            .batch_size(1000)
            .await?;
        Ok(cursor.try_collect().await?)
    }

    /// 找出这批 id 里已经入库的部分
    pub async fn existing_ids<T>(&self, name: &str, ids: &[T]) -> Result<Vec<bson::Bson>>
    where
        T: Serialize,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let values = ids
            .iter()
            .map(|id| bson::serialize_to_bson(id).map_err(Error::from))
            .collect::<Result<Vec<_>>>()?;

        let collection = self.collection::<Document>(name);
        let cursor = collection
            .find(doc! { "_id": { "$in": values } })
            .projection(doc! { "_id": 1 })
            .await?;
        let documents: Vec<Document> = cursor.try_collect().await?;
        Ok(documents
            .into_iter()
            .filter_map(|d| d.get("_id").cloned())
            .collect())
    }

    pub async fn delete_by_id<T: Serialize>(&self, name: &str, id: &T) -> Result<u64> {
        let value = bson::serialize_to_bson(id)?;
        let result = self
            .collection::<Document>(name)
            .delete_one(doc! { "_id": value })
            .await?;
        Ok(result.deleted_count)
    }

    pub async fn delete_many(&self, name: &str, filter: Document) -> Result<u64> {
        let result = self.collection::<Document>(name).delete_many(filter).await?;
        Ok(result.deleted_count)
    }

    /// 建立本仓库写入、mcim-rust-api 读取所需的索引
    ///
    /// Python 版只在模型里声明 index=True 却从不调用建索引，实际靠人工维护。
    /// 缺了这些索引，同步时按 modId / project_id 的删除会退化成全表扫描
    pub async fn ensure_indexes(&self) -> Result<Vec<String>> {
        use mongodb::IndexModel;

        let plan: &[(&str, &str)] = &[
            // 同步时按 mod 清理旧文件，读侧按 modId 取文件列表
            ("curseforge_files", "modId"),
            // 读侧按指纹反查
            ("curseforge_files", "fileFingerprint"),
            ("curseforge_categories", "gameId"),
            // 读侧按 hash 反查版本
            ("modrinth_files", "_id.sha1"),
            ("modrinth_files", "_id.sha512"),
            // 同步时按项目清理旧文件与旧版本
            ("modrinth_files", "project_id"),
            ("modrinth_files", "version_id"),
            ("modrinth_versions", "project_id"),
        ];

        let mut created = Vec::new();
        for (collection, field) in plan {
            let model = IndexModel::builder()
                .keys(doc! { *field: 1 })
                .options(
                    mongodb::options::IndexOptions::builder()
                        .name(Some(format!("{}_1", field)))
                        .build(),
                )
                .build();
            let name = self
                .collection::<Document>(collection)
                .create_index(model)
                .await?
                .index_name;
            created.push(format!("{}.{}", collection, name));
        }
        Ok(created)
    }

    pub async fn count(&self, name: &str) -> Result<u64> {
        Ok(self
            .collection::<Document>(name)
            .count_documents(doc! {})
            .await?)
    }
}
