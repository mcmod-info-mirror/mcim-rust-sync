use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use backon::{ExponentialBuilder, Retryable};
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed};
use governor::{Quota, RateLimiter};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::config::Config;
use crate::error::{Error, Result};

type DomainLimiter = RateLimiter<NotKeyed, InMemoryState, DefaultClock>;

/// Modrinth 要求带能识别来源的 User-Agent
const USER_AGENT_VALUE: &str = concat!(
    "mcim-rust-sync/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/mcmod-info-mirror/mcim-rust-sync)"
);

const MAX_ERROR_BODY: usize = 300;

/// 单次请求的总耗时上限
///
/// reqwest 默认不超时，对端不回也不断开时请求会一直挂着，退避重试根本等不到。
/// 常驻运行时这会把任务的槽位永久占住，之后每轮都被重叠保护跳过
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

pub struct HttpClient {
    client: reqwest::Client,
    /// 按域名限速，未配置的域名不限速
    limiters: HashMap<String, Arc<DomainLimiter>>,
    max_retries: usize,
}

impl HttpClient {
    pub fn new(config: &Config) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));

        let mut builder = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT);
        if let Some(proxy) = config.proxy.as_deref().filter(|p| !p.is_empty()) {
            builder = builder.proxy(reqwest::Proxy::all(proxy)?);
        }

        let mut limiters = HashMap::new();
        for (domain, limit) in &config.domain_rate_limits {
            let refill = NonZeroU32::new(limit.refill_rate.max(1))
                .expect("refill_rate 已保证非零");
            let capacity = NonZeroU32::new(limit.capacity.max(1))
                .expect("capacity 已保证非零");
            let quota = Quota::per_second(refill).allow_burst(capacity);
            limiters.insert(domain.clone(), Arc::new(RateLimiter::direct(quota)));
        }

        Ok(Self {
            client: builder.build()?,
            limiters,
            max_retries: 3,
        })
    }

    pub async fn get<T: DeserializeOwned>(
        &self,
        url: &str,
        query: &[(&str, String)],
        headers: &HeaderMap,
    ) -> Result<T> {
        self.send(Method::GET, url, query, None::<&()>, headers).await
    }

    pub async fn post<T: DeserializeOwned, B: Serialize>(
        &self,
        url: &str,
        body: &B,
        headers: &HeaderMap,
    ) -> Result<T> {
        self.send(Method::POST, url, &[], Some(body), headers).await
    }

    async fn send<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        url: &str,
        query: &[(&str, String)],
        body: Option<&B>,
        headers: &HeaderMap,
    ) -> Result<T> {
        let attempt = || async {
            self.acquire(url).await;

            let mut request = self
                .client
                .request(method.clone(), url)
                .headers(headers.clone());
            if !query.is_empty() {
                request = request.query(query);
            }
            if let Some(body) = body {
                request = request.json(body);
            }

            let response = request.send().await?;
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                return Err(Error::ResponseCode {
                    status: status.as_u16(),
                    method: method.to_string(),
                    url: url.to_string(),
                    body: truncate(&body, MAX_ERROR_BODY),
                });
            }

            Ok(response.json::<T>().await?)
        };

        attempt
            .retry(
                ExponentialBuilder::default()
                    .with_max_times(self.max_retries)
                    .with_jitter(),
            )
            // 与 Python 版不同，429 与 5xx 都会退避重试
            .when(|e: &Error| e.is_retryable())
            .notify(|e: &Error, delay| {
                tracing::warn!(error = %e, ?delay, "请求失败，退避后重试");
            })
            .await
    }

    /// 取令牌，令牌不足时异步等待而不是占住线程
    async fn acquire(&self, url: &str) {
        let Ok(parsed) = reqwest::Url::parse(url) else {
            return;
        };
        let Some(host) = parsed.host_str() else {
            return;
        };
        if let Some(limiter) = self.limiters.get(host) {
            limiter.until_ready().await;
        }
    }
}

/// 截断字符串，超长时加省略号
fn truncate(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    value.chars().take(limit).collect::<String>() + "..."
}
