//! 网络层错误分类。
//!
//! 上层的重试、回退、放弃决策只看 [`Error::is_retryable`]，不解析错误字符串。
//! 面向用户的文案不带 query；完整 URL 只经 `proto::log::debug` 写入 launcher.log。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// 传输失败（DNS/TCP/TLS/超时/读响应体）。
    Transport(String),
    /// HTTP 状态码非 200（`message` 保留各接口原有的用户文案）。
    Http { status: u16, message: String },
    /// 响应解析失败（JSON/PNG/区服表结构）。
    Parse(String),
    /// 服务端明确拒绝：重试无用。
    Rejected(String),
}

impl Error {
    /// 是否值得重试。`Http` 也算：这些接口的非 200 都是“服务繁忙”语义。
    pub fn is_retryable(&self) -> bool {
        !matches!(self, Error::Rejected(_))
    }

    pub fn transport(message: impl Into<String>) -> Error {
        Error::Transport(message.into())
    }

    pub fn http(status: u16, message: impl Into<String>) -> Error {
        Error::Http {
            status,
            message: message.into(),
        }
    }

    pub fn parse(message: impl Into<String>) -> Error {
        Error::Parse(message.into())
    }

    pub fn rejected(message: impl Into<String>) -> Error {
        Error::Rejected(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Transport(m) | Error::Parse(m) | Error::Rejected(m) => write!(f, "{m}"),
            Error::Http { message, .. } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_rejected_is_not_retryable() {
        assert!(Error::transport("x").is_retryable());
        assert!(Error::http(503, "x").is_retryable());
        assert!(Error::parse("x").is_retryable());
        assert!(!Error::rejected("x").is_retryable());
    }

    #[test]
    fn http_display_uses_original_message() {
        let e = Error::http(500, "登录服务繁忙（HTTP 500），请重试");
        assert_eq!(e.to_string(), "登录服务繁忙（HTTP 500），请重试");
        assert_eq!(e, Error::Http { status: 500, message: "登录服务繁忙（HTTP 500），请重试".into() });
    }
}
