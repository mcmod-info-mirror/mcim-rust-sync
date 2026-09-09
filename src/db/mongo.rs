use std::collections::BTreeMap;

use bson::{Document, doc};
use chrono::{DateTime, Utc};
use futures::Stream;
use futures::stream::{self, StreamExt, TryStreamExt};
use mongodb::options::ClientOptions;
use mongodb::{Client, Collection};
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::models::collection;
use crate::models::task_run::TaskRun;

#[derive(Clone)]
pub struct Database {
    inner: mongodb::Database,
}

/// `GET /api/freshness` 的响应体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessResponse {
    pub generated_at: DateTime<Utc>,
    pub collections: BTreeMap<String, FreshnessCollection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreshnessCollection {
    pub total: u64,
    pub checked_within: CheckedWithin,
    pub never_checked: u64,
    pub oldest_checked_at: Option<DateTime<Utc>>,
    pub newest_sync_at: Option<DateTime<Utc>>,
}

/// 键是 `2h` / `24h` / `7d`，不是合法 Rust 标识符，所以用手写 rename
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckedWithin {
    #[serde(rename = "2h")]
    pub h2: u64,
    #[serde(rename = "24h")]
    pub h24: u64,
    #[serde(rename = "7d")]
    pub d7: u64,
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
            .batch_size(1000)
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

    pub async fn insert_task_run(&self, run: &TaskRun) -> Result<()> {
        self.collection::<TaskRun>(collection::TASK_RUNS)
            .insert_one(run)
            .await?;
        Ok(())
    }

    pub async fn update_task_run(&self, run: &TaskRun) -> Result<()> {
        self.collection::<TaskRun>(collection::TASK_RUNS)
            .replace_one(doc! { "_id": run.id }, run)
            .await?;
        Ok(())
    }

    pub async fn mark_running_task_runs_interrupted(
        &self,
        finished_at: DateTime<Utc>,
    ) -> Result<u64> {
        let result = self
            .collection::<Document>(collection::TASK_RUNS)
            .update_many(
                doc! { "status": "running" },
                doc! {
                    "$set": {
                        "status": "interrupted",
                        "finished_at": bson::DateTime::from_chrono(finished_at),
                        "error": "process restarted before task completion"
                    }
                },
            )
            .await?;
        Ok(result.modified_count)
    }

    pub async fn list_task_runs(
        &self,
        task: Option<&str>,
        status: Option<&str>,
        started_after: Option<DateTime<Utc>>,
        started_before: Option<DateTime<Utc>>,
        limit: i64,
    ) -> Result<Vec<TaskRun>> {
        let mut filter = Document::new();
        if let Some(task) = task {
            filter.insert("task", task);
        }
        if let Some(status) = status {
            filter.insert("status", status);
        }
        if started_after.is_some() || started_before.is_some() {
            let mut range = Document::new();
            if let Some(value) = started_after {
                range.insert("$gte", bson::DateTime::from_chrono(value));
            }
            if let Some(value) = started_before {
                range.insert("$lte", bson::DateTime::from_chrono(value));
            }
            filter.insert("started_at", range);
        }
        let cursor = self
            .collection::<TaskRun>(collection::TASK_RUNS)
            .find(filter)
            .sort(doc! { "started_at": -1 })
            .limit(limit.clamp(1, 500))
            .await?;
        Ok(cursor.try_collect().await?)
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
            ("curseforge_mods", doc! { "sync_at": 1 }, "sync_at_1"),
            (
                "modrinth_projects",
                doc! { "sync_at": 1 },
                "sync_at_1",
            ),
            ("modrinth_files", doc! { "_id.sha1": 1 }, "_id.sha1_1"),
            ("modrinth_files", doc! { "_id.sha512": 1 }, "_id.sha512_1"),
            ("modrinth_files", doc! { "version_id": 1 }, "version_id_1"),
            (
                "modrinth_files",
                doc! { "project_id": 1, "version_id": 1, "filename": 1 },
                "project_id_1_version_id_1_filename_1",
            ),
            (
                "task_runs",
                doc! { "task": 1, "started_at": -1 },
                "task_1_started_at_-1",
            ),
            (
                "task_runs",
                doc! { "status": 1, "started_at": -1 },
                "status_1_started_at_-1",
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

    /// 两个主集合的新鲜度汇总
    ///
    /// 一次调用同时算 `curseforge_mods` 与 `modrinth_projects`，两个集合并行
    pub async fn freshness_summary(&self, now: DateTime<Utc>) -> Result<FreshnessResponse> {
        let curseforge = self.freshness(collection::CURSEFORGE_MODS, now);
        let modrinth = self.freshness(collection::MODRINTH_PROJECTS, now);
        let (curseforge, modrinth) = tokio::join!(curseforge, modrinth);

        let mut collections = BTreeMap::new();
        collections.insert(collection::CURSEFORGE_MODS.to_string(), curseforge?);
        collections.insert(collection::MODRINTH_PROJECTS.to_string(), modrinth?);

        Ok(FreshnessResponse {
            generated_at: now,
            collections,
        })
    }

    /// 单个集合的「核对新鲜度」统计
    ///
    /// 时间阈值计数走 `checked_at_1` 索引；`total` 用元数据计数（O(1)、近似）；
    /// `never_checked` 与 `oldest_checked_at` 用 `$gte: epoch` 的范围走索引，
    /// 避免 `$exists` 触发的全表扫；`newest_sync_at` 走 `sync_at_1` 索引降序取第一条
    async fn freshness(&self, name: &str, now: DateTime<Utc>) -> Result<FreshnessCollection> {
        let collection = self.collection::<Document>(name);
        let window_2h = now - chrono::Duration::hours(2);
        let window_24h = now - chrono::Duration::hours(24);
        let window_7d = now - chrono::Duration::days(7);
        // epoch 起算点：`checked_at_1` 是非稀疏索引，缺失字段在库里记成 `null`；
        // 用 `$gte: epoch` 走索引范围、天然排除 `null`/缺失，比 `$exists` 全表扫快得多
        let epoch = DateTime::<Utc>::UNIX_EPOCH;

        let total_collection = collection.clone();
        let checked_collection = collection.clone();
        let checked_24h_collection = collection.clone();
        let checked_7d_collection = collection.clone();
        let checked_epoch_collection = collection.clone();
        let (total, checked_2h, checked_24h, checked_7d, checked_total) = tokio::join!(
            // 元数据计数，O(1)；代价是近似值，可能轻微滞后于最近的写入
            total_collection.estimated_document_count(),
            checked_collection.count_documents(
                doc! { "checked_at": { "$gte": bson::DateTime::from_chrono(window_2h) } }
            ),
            checked_24h_collection.count_documents(
                doc! { "checked_at": { "$gte": bson::DateTime::from_chrono(window_24h) } }
            ),
            checked_7d_collection.count_documents(
                doc! { "checked_at": { "$gte": bson::DateTime::from_chrono(window_7d) } }
            ),
            // 已核对过的数量：epoch 到现在的范围计数
            checked_epoch_collection.count_documents(
                doc! { "checked_at": { "$gte": bson::DateTime::from_chrono(epoch) } }
            ),
        );
        let total = total?;
        let checked_2h = checked_2h?;
        let checked_24h = checked_24h?;
        let checked_7d = checked_7d?;
        let checked_total = checked_total?;
        // never_checked = 总数 − 已核对数，避免 `$exists:false` 的全表扫
        let never_checked = total.saturating_sub(checked_total);

        let oldest_checked_at = collection
            .clone()
            // epoch 起算的升序第一条 = 最早的已核对时间；`$gte: epoch` 走索引，
            // 且不会命中缺失字段（它们等于 `null`、排在 epoch 之前）
            .find_one(doc! { "checked_at": { "$gte": bson::DateTime::from_chrono(epoch) } })
            .sort(doc! { "checked_at": 1 })
            .projection(doc! { "checked_at": 1 })
            .await?
            .and_then(|document| {
                document
                    .get_datetime("checked_at")
                    .map(|value| value.to_chrono())
                    .ok()
            });

        // `sync_at` 索引加上后，降序取第一条即最大值，不必全表 `$max`
        let newest_sync_at = collection
            .clone()
            .find_one(doc! {})
            .sort(doc! { "sync_at": -1 })
            .projection(doc! { "sync_at": 1 })
            .await?
            .and_then(|document| {
                document
                    .get_datetime("sync_at")
                    .map(|value| value.to_chrono())
                    .ok()
            });

        Ok(FreshnessCollection {
            total,
            checked_within: CheckedWithin {
                h2: checked_2h,
                h24: checked_24h,
                d7: checked_7d,
            },
            never_checked,
            oldest_checked_at,
            newest_sync_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freshness_serializes_with_contract_keys() {
        let now = Utc::now();
        let mut collections = BTreeMap::new();
        collections.insert(
            collection::CURSEFORGE_MODS.to_string(),
            FreshnessCollection {
                total: 3,
                checked_within: CheckedWithin {
                    h2: 1,
                    h24: 2,
                    d7: 3,
                },
                never_checked: 1,
                oldest_checked_at: Some(now),
                newest_sync_at: Some(now),
            },
        );

        let value = serde_json::to_value(FreshnessResponse {
            generated_at: now,
            collections,
        })
        .expect("freshness 响应可序列化");

        let item = &value["collections"]["curseforge_mods"];
        assert_eq!(item["total"], 3);
        assert_eq!(item["never_checked"], 1);
        assert_eq!(item["checked_within"]["2h"], 1);
        assert_eq!(item["checked_within"]["24h"], 2);
        assert_eq!(item["checked_within"]["7d"], 3);
        assert!(item["oldest_checked_at"].is_string());
        assert!(item["newest_sync_at"].is_string());
    }
}
