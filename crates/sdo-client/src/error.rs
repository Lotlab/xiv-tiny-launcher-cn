//! 网络层错误分类。
//!
//! 每个错误分两部分：
//!
//! - [`Error::user`]：给用户看的文案（不带 query、设备指纹与技术细节）；
//! - [`Error::detail`]：排查细节，只进日志（见 [`Error::log_text`]）。
//!
//! [`Display`](std::fmt::Display) 只输出 `user`，因此 `Error` 转成上层错误或直接
//! `println!` 都不会泄漏细节；上层的重试、回退、放弃决策只看 [`Error::is_retryable`]，
//! 不解析错误字符串。

/// 失败类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 传输失败（DNS/TCP/TLS/超时/读响应体）。
    Transport,
    /// HTTP 状态码非 200。
    Http(u16),
    /// 响应解析失败（JSON/PNG/区服表结构）。
    Parse,
    /// 服务端明确拒绝：重试无用。
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    kind: Kind,
    user: String,
    detail: String,
}

/// 传输失败的用户文案（细节在 `detail`）。
const USER_TRANSPORT: &str = "网络请求失败，请重试";
/// 解析失败的用户文案（需要更具体时用 [`Error::with_user`] 覆盖）。
const USER_PARSE: &str = "服务端响应异常，请重试";

impl Error {
    fn new(kind: Kind, user: impl Into<String>, detail: impl Into<String>) -> Error {
        Error {
            kind,
            user: user.into(),
            detail: detail.into(),
        }
    }

    /// 传输失败：用户文案统一，细节进日志。
    pub fn transport(detail: impl Into<String>) -> Error {
        Error::new(Kind::Transport, USER_TRANSPORT, detail)
    }

    /// HTTP 状态码非 200；`user` 是给用户的文案。
    pub fn http(status: u16, user: impl Into<String>) -> Error {
        Error::new(Kind::Http(status), user, format!("HTTP {status}"))
    }

    /// 响应解析失败：用户文案统一，细节进日志。
    pub fn parse(detail: impl Into<String>) -> Error {
        Error::new(Kind::Parse, USER_PARSE, detail)
    }

    /// 服务端明确拒绝；`user` 通常是服务端返回的原文。
    pub fn rejected(user: impl Into<String>) -> Error {
        let user = user.into();
        Error::new(Kind::Rejected, user.clone(), user)
    }

    /// 覆盖用户文案（解析类错误需要给具体提示时用）。
    pub fn with_user(mut self, user: impl Into<String>) -> Error {
        self.user = user.into();
        self
    }

    /// 覆盖排查细节。
    pub fn with_detail(mut self, detail: impl Into<String>) -> Error {
        self.detail = detail.into();
        self
    }

    /// 是否值得重试。`Http` 也算：这些接口的非 200 都是“服务繁忙”语义。
    pub fn is_retryable(&self) -> bool {
        !matches!(self.kind, Kind::Rejected)
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// 面向用户的文案（[`Display`](std::fmt::Display) 同）。
    pub fn user(&self) -> &str {
        &self.user
    }

    /// 排查细节。
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// 写日志用的一行：`user | detail`（两者相同时只留一份）。
    pub fn log_text(&self) -> String {
        if self.detail.is_empty() || self.detail == self.user {
            self.user.clone()
        } else {
            format!("{} | {}", self.user, self.detail)
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.user)
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
        assert_eq!(Error::http(503, "x").kind(), Kind::Http(503));
    }

    /// 用户文案与排查细节分离：`Display` 只给用户文案（URL/解析器报错不得进），
    /// 细节只走 `log_text()`，且可以用 `with_user` 覆盖用户文案。
    #[test]
    fn user_text_and_detail_stay_separate() {
        let e = Error::transport(
            "请求失败 https://cas.sdo.com/authen/getGuid.json: connection reset",
        );
        assert_eq!(e.to_string(), "网络请求失败，请重试");
        assert!(!e.to_string().contains("cas.sdo.com"));
        assert!(e.log_text().contains("connection reset"), "{}", e.log_text());

        let p = Error::parse("expected value at line 1 column 1")
            .with_user("区服列表已损坏，请删除 server.json 后重试");
        assert_eq!(p.to_string(), "区服列表已损坏，请删除 server.json 后重试");
        assert!(p.log_text().contains("line 1"), "{}", p.log_text());

        // 细节与用户文案相同时不重复（rejected 两边同源）
        assert_eq!(Error::rejected("换票失败：x").log_text(), "换票失败：x");
        assert_eq!(
            Error::rejected("需要人脸验证").with_detail("openFace=1").log_text(),
            "需要人脸验证 | openFace=1"
        );

        // http() 的用户文案就是调用方传的那句（不得改写/前缀状态码）
        let h = Error::http(500, "登录服务繁忙（HTTP 500），请重试");
        assert_eq!(h.to_string(), "登录服务繁忙（HTTP 500），请重试");
        assert_eq!(h.detail(), "HTTP 500");
        assert_eq!(h.kind(), Kind::Http(500));
    }
}
