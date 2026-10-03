//! 最小 HTTP 客户端：GET + 固定超时。

use std::net::ToSocketAddrs;
use std::sync::OnceLock;
use std::time::Duration;

use proto::consts::{ACCEPT, TIMEOUT_AUTH_MS, TIMEOUT_DOWNLOAD_MS, UA};

#[derive(Debug)]
pub struct Resp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Resp {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> Result<serde_json::Value, String> {
        serde_json::from_slice(&self.body).map_err(|e| format!("JSON 解析失败: {e}"))
    }

    /// 大小写不敏感的响应头取值（同名多值全部返回）。
    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    /// CAS 认证接口（短超时）。
    Auth,
    /// 下载类（长超时）。
    Download,
}

impl Timeout {
    fn ms(self) -> u64 {
        match self {
            Timeout::Auth => TIMEOUT_AUTH_MS,
            Timeout::Download => TIMEOUT_DOWNLOAD_MS,
        }
    }
}

/// `GET https://{host}{path_query}`。
pub fn get(host: &str, path_query: &str, timeout: Timeout) -> Result<Resp, String> {
    match (host, 443u16).to_socket_addrs() {
        Ok(mut it) => {
            if it.next().is_none() {
                return Err(format!("网络异常，域名无法解析：{host}"));
            }
        }
        Err(e) => return Err(format!("网络异常，域名无法解析：{host}（{e}）")),
    };

    let ms = timeout.ms();
    // SAFETY: 认证链自签，服务端证书无法通过系统根校验，关闭校验是登录必需。
    // 连接复用：超时按请求设置，Client 全局共享。
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_millis(TIMEOUT_AUTH_MS))
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true)
            .no_proxy()
            .build()
            .expect("HTTP 客户端初始化失败")
    });

    let url = format!("https://{host}{path_query}");
    let mut self_headers = reqwest::header::HeaderMap::new();
    self_headers.insert(
        reqwest::header::HOST,
        reqwest::header::HeaderValue::from_str(host).map_err(|e| format!("非法 Host 头: {e}"))?,
    );
    self_headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_str(UA).map_err(|e| format!("非法 UA 头: {e}"))?,
    );
    self_headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_str(ACCEPT)
            .map_err(|e| format!("非法 Accept 头: {e}"))?,
    );
    let req = client
        .get(&url)
        .headers(self_headers)
        .timeout(Duration::from_millis(ms));

    let resp = req.send().map_err(|e| format!("请求失败 {url}: {e}"))?;
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let body = resp
        .bytes()
        .map_err(|e| format!("读取响应体失败 {url}: {e}"))?
        .to_vec();
    Ok(Resp {
        status,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_lookup_is_case_insensitive() {
        let r = Resp {
            status: 200,
            headers: vec![
                ("Set-Cookie".into(), "CODEKEY=abc;Path=/".into()),
                ("set-cookie".into(), "CODEKEY_COUNT=1;Path=/".into()),
            ],
            body: vec![],
        };
        assert_eq!(r.header_values("SET-COOKIE").len(), 2);
        assert_eq!(
            proto::resp::extract_codekey(r.header_values("set-cookie")).unwrap(),
            "abc"
        );
    }

    #[test]
    fn dns_precheck_fails_fast_for_bogus_host() {
        let err = get("this-host-does-not-exist.invalid", "/x", Timeout::Auth).unwrap_err();
        assert!(err.contains("无法解析"), "{err}");
    }
}
