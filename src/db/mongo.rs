use bson::{Document, doc};
use chrono::{DateTime, Utc};
use futures::Stream;
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
        let database: mongodb::Database = client.database(&config.mongodb.database);

        // 连不上就直接失败
        database.run_command(doc! { "ping": 1 }).await?;

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

        let collection = self.collection::<Document>(name);
        let mut written = 0u64;
        // Do not serialize the whole input slice at once. Large Modrinth projects
        // contain thousands of files and the serialized BSON duplicates the input.
        for batch in items.chunks(64) {
            let documents = batch
                .iter()
                .map(|item| bson::serialize_to_document(item).map_err(Error::from))
                .collect::<Result<Vec<Document>>>()?;

            written += stream::iter(documents)
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
                .await?;
        }
        Ok(written)
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

    /// 分块遍历整个集合，只取需要的字段
    ///
    /// 取代 Python 版无排序的 skip/limit 分页，后者在并发写入时会漏读或重读。
    /// 不能一次性 collect 成 Vec：`modrinth_projects` 连 versions 与
    /// game_versions 两个数组有两百多 MB，几个刷新任务并发就把容器撑爆
    pub async fn chunked_all<T>(
        &self,
        name: &str,
        projection: Document,
        size: usize,
    ) -> Result<impl Stream<Item = Result<Vec<T>>> + Unpin>
    where
        T: DeserializeOwned + Send + Sync,
    {
        let cursor = self
            .collection::<T>(name)
            .find(doc! {})
            .projection(projection)
            .batch_size(size.max(1) as u32)
            .await?;

        Ok(Box::pin(Box::pin(cursor).chunks(size.max(1)).map(
            |batch| {
                batch
                    .into_iter()
                    .collect::<std::result::Result<Vec<T>, _>>()
                    .map_err(Error::from)
            },
        )))
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

    /// 标记这批文档在此刻被核对过
    ///
    /// 走 `$set` 而不是整文档替换，比对时绝大多数条目内容没变，
    /// 没必要为了一个时间戳把整份文档重写一遍
    pub async fn touch_checked<T>(&self, name: &str, ids: &[T], at: DateTime<Utc>) -> Result<u64>
    where
        T: Serialize,
    {
        if ids.is_empty() {
            return Ok(0);
        }
        let values = ids
            .iter()
            .map(|id| bson::serialize_to_bson(id).map_err(Error::from))
            .collect::<Result<Vec<_>>>()?;

        let result = self
            .collection::<Document>(name)
            .update_many(
                doc! { "_id": { "$in": values } },
                doc! { "$set": { "checked_at": bson::DateTime::from_chrono(at) } },
            )
            .await?;
        Ok(result.modified_count)
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
        let result = self
            .collection::<Document>(name)
            .delete_many(filter)
            .await?;
        Ok(result.deleted_count)
    }

    pub async fn ensure_indexes(&self) -> Result<Vec<String>> {
        use mongodb::IndexModel;
        use mongodb::options::IndexOptions;

        let plan: &[(&str, Document, &str)] = &[
            ("curseforge_files", doc! { "modId": 1 }, "modId_1"),
            (
                "curseforge_files",
                doc! { "fileFingerprint": 1 },
                "fileFingerprint_1",
            ),
            ("curseforge_categories", doc! { "gameId": 1 }, "gameId_1"),
            ("modrinth_projects", doc! { "slug": 1 }, "slug_1"),
            (
                "modrinth_versions",
                doc! { "project_id": 1 },
                "project_id_1",
            ),
            ("curseforge_mods", doc! { "checked_at": 1 }, "checked_at_1"),
            (
                "modrinth_projects",
                doc! { "checked_at": 1 },
                "checked_at_1",
            ),
            ("modrinth_files", doc! { "_id.sha1": 1 }, "_id.sha1_1"),
            ("modrinth_files", doc! { "_id.sha512": 1 }, "_id.sha512_1"),
            ("modrinth_files", doc! { "version_id": 1 }, "version_id_1"),
            (
                "modrinth_files",
                doc! { "project_id": 1, "version_id": 1, "filename": 1 },
                "project_id_1_version_id_1_filename_1",
            ),
        ];

        let mut created = Vec::with_capacity(plan.len());
        for &(collection, ref keys, name) in plan {
            let model = IndexModel::builder()
                .keys(keys.clone())
                .options(IndexOptions::builder().name(Some(name.to_string())).build())
                .build();

            let index_name = self
                .collection::<Document>(collection)
                .create_index(model)
                .await?
                .index_name;

            created.push(format!("{}.{}", collection, index_name));
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
