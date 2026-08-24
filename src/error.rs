use thiserror::Error;

/// 同步过程中可能出现的错误
///
/// 与 Python 版不同，这里不吞异常：所有失败都必须显式处理或向上传播
#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),

    #[error("[{method}] {status} {url} {body}")]
    ResponseCode {
        status: u16,
        method: String,
        url: String,
        body: String,
    },

    #[error(transparent)]
    Request(#[from] reqwest::Error),

    #[error(transparent)]
    Mongo(#[from] mongodb::error::Error),

    #[error(transparent)]
    Bson(#[from] bson::error::Error),

    #[error(transparent)]
    Redis(#[from] redis::RedisError),

    #[error(transparent)]
    Json(#[from] serde_json::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// 上游返回的 HTTP 状态码
    pub fn status(&self) -> Option<u16> {
        match self {
            Error::ResponseCode { status, .. } => Some(*status),
            Error::Request(e) => e.status().map(|s| s.as_u16()),
            _ => None,
        }
    }

    /// 资源不存在，调用方应当跳过而不是重试
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }

    /// 是否值得重试
    ///
    /// Python 版把 429 归进 `ResponseCodeException` 又整类排除出重试，
    /// 导致触发限速直接失败，这里明确把 429 与 5xx 纳入重试
    pub fn is_retryable(&self) -> bool {
        match self.status() {
            Some(429) => true,
            Some(s) if s >= 500 => true,
            Some(_) => false,
            None => matches!(self, Error::Request(_)),
        }
    }
}
