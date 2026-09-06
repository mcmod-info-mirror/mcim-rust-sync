use std::collections::BTreeMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use chrono::{DateTime, TimeZone, Utc};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use tokio::net::TcpListener;

use crate::db::Database;
use crate::error::Error;

#[derive(Serialize)]
struct TaskRunsResponse<T> {
    data: T,
    count: usize,
}

#[derive(Clone, Default)]
pub struct ScheduleState {
    next_runs: Arc<RwLock<BTreeMap<String, DateTime<Utc>>>>,
}

#[derive(serde::Serialize)]
struct ScheduledTask {
    task: String,
    next_run_at: DateTime<Utc>,
}

impl ScheduleState {
    pub fn replace(&self, jobs: impl IntoIterator<Item = (String, DateTime<Utc>)>) {
        let mut next_runs = self
            .next_runs
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        next_runs.clear();
        next_runs.extend(jobs);
    }

    pub fn set_next(&self, task: &str, next: DateTime<Utc>) {
        self.next_runs
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(task.to_string(), next);
    }

    fn snapshot(&self) -> Vec<ScheduledTask> {
        self.next_runs
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(task, next_run_at)| ScheduledTask {
                task: task.clone(),
                next_run_at: *next_run_at,
            })
            .collect()
    }
}

pub async fn serve(db: Database, addr: SocketAddr, schedule: ScheduleState) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "task history API listening");
    loop {
        let (stream, _) = listener.accept().await?;
        let db = db.clone();
        let schedule = schedule.clone();
        tokio::spawn(async move {
            let service =
                service_fn(move |request| response(db.clone(), schedule.clone(), request));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                tracing::debug!(%error, "task API connection closed");
            }
        });
    }
}

async fn response(
    db: Database,
    schedule: ScheduleState,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let result = match request.uri().path() {
        "/healthz" => json(StatusCode::OK, serde_json::json!({"status": "ok"})),
        "/api/task-runs" => task_runs(&db, request.uri().query()).await,
        "/api/tasks" => json(
            StatusCode::OK,
            serde_json::json!({"data": schedule.snapshot()}),
        ),
        _ => json(
            StatusCode::NOT_FOUND,
            serde_json::json!({"error": "not found"}),
        ),
    };
    Ok(result)
}

async fn task_runs(db: &Database, query: Option<&str>) -> Response<Full<Bytes>> {
    let params = query.map(parse_query).unwrap_or_default();
    let task = params.get("task").map(String::as_str);
    let status = params.get("status").map(String::as_str);
    let started_after = timestamp_param(params.get("from"));
    let started_before = timestamp_param(params.get("to"));
    let limit = params
        .get("limit")
        .and_then(|value| value.parse::<i64>().ok())
        .unwrap_or(100);

    match db
        .list_task_runs(task, status, started_after, started_before, limit)
        .await
    {
        Ok(data) => json(
            StatusCode::OK,
            serde_json::to_value(TaskRunsResponse {
                count: data.len(),
                data,
            })
            .unwrap_or_else(|_| serde_json::json!({"error": "serialization failed"})),
        ),
        Err(error) => json(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": public_error(&error)}),
        ),
    }
}

fn timestamp_param(value: Option<&String>) -> Option<chrono::DateTime<Utc>> {
    let millis = value?.parse::<i64>().ok()?;
    Utc.timestamp_millis_opt(millis).single()
}

fn json(status: StatusCode, value: serde_json::Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("access-control-allow-origin", "*")
        .body(Full::new(Bytes::from(value.to_string())))
        .expect("valid HTTP response")
}

fn parse_query(raw: &str) -> std::collections::HashMap<String, String> {
    raw.split('&')
        .filter_map(|part| part.split_once('='))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

fn public_error(error: &Error) -> String {
    match error {
        Error::Mongo(_) | Error::Bson(_) => "database error".to_string(),
        _ => "request failed".to_string(),
    }
}
