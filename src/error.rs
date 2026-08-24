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

#[cfg(test)]
mod tests {
    use super::Error;

    fn response(status: u16) -> Error {
        Error::ResponseCode {
            status,
            method: "GET".to_string(),
            url: "https://example.invalid".to_string(),
            body: String::new(),
        }
    }

    /// Python 版把 429 归进 ResponseCodeException 又整类排除出重试，
    /// 结果一撞限速就直接失败
    #[test]
    fn rate_limited_is_retryable() {
        assert!(response(429).is_retryable());
    }

    #[test]
    fn server_errors_are_retryable() {
        assert!(response(500).is_retryable());
        assert!(response(502).is_retryable());
        assert!(response(503).is_retryable());
    }

    #[test]
    fn client_errors_are_not_retryable() {
        assert!(!response(400).is_retryable());
        assert!(!response(403).is_retryable());
        assert!(!response(404).is_retryable());
    }

    /// 404 要能和「同步失败」区分开，否则不存在的条目会被反复重试
    #[test]
    fn not_found_is_distinguishable() {
        assert!(response(404).is_not_found());
        assert!(!response(403).is_not_found());
        assert!(!response(500).is_not_found());
        assert!(!Error::Config("x".to_string()).is_not_found());
    }

    #[test]
    fn local_errors_are_not_retryable() {
        assert!(!Error::Config("配置有问题".to_string()).is_retryable());
    }
}
