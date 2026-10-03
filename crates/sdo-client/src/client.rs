//! 最小 HTTP 传输层 + 会话客户端。传输口径见 `DESIGN.md` 第 5 节。
//!
//! - 每请求新建内核（禁止复用），`Host / User-Agent / Accept` 三行头，禁 `Referer`，
//!   Cookie jar 禁用（只取 `CODEKEY`，见 [`crate::resp`]）。
//! - 成功条件（仅 `HTTP 200`；`200 + return_code != 0` 按失败走各自分支）由各业务方法判定，
//!   本层只透传状态码与原文；日志脱敏由调用方经 `proto::log::sanitize` 处理。

use std::net::ToSocketAddrs;
use std::time::Duration;

use proto::consts::{ACCEPT, TIMEOUT_AUTH_MS, TIMEOUT_DOWNLOAD_MS, UA};

use crate::endpoint::{Endpoint, LoginApp};

pub type Result<T> = std::result::Result<T, String>;

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

    pub fn json(&self) -> Result<serde_json::Value> {
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

/// 网络身份：`Client` 唯一关心的设备字段（会话内不变）。
///
/// 刻意不持有整个 [`proto::device::Device`]，只快照网络需要的四个字段；
/// 落盘字段（`keepLoginKey/lastAreaId`）由 EXE 侧管理，不进本库视野。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// `{SEG0}:{SEG1}:`（第三段空，保留末尾冒号原文发送）。
    pub device_id: String,
    /// `XX-XX-XX-XX-XX-XX`，大写连字符。
    pub mac_id: String,
    /// 首个私网 IPv4，会话内不变。
    pub ep_ip: String,
    /// `DESKTOP-` + 7 位大写字母数字（发送时做 URL 编码）。
    pub ep_name: String,
}

impl From<&proto::device::Device> for Identity {
    fn from(d: &proto::device::Device) -> Identity {
        Identity {
            device_id: d.device_id.clone(),
            mac_id: d.mac_id.clone(),
            ep_ip: d.ep_ip.clone(),
            ep_name: d.ep_name.clone(),
        }
    }
}

/// SDO 网络客户端：一次登录会话的网络身份
/// （设备快照 + 本进程 `runTimeId` + 登录应用配置）。
///
/// 构造时克隆快照；游戏应用作用域（[`GameApp`](crate::endpoint::GameApp)）不是客户端成分，
/// 只在换票时按次传入。
#[derive(Debug, Clone)]
pub struct Client {
    identity: Identity,
    run_time_id: String,
    login: LoginApp,
}

impl Client {
    /// 按冻结口径（`proto::consts` 默认值）构造。
    pub fn new(identity: Identity, run_time_id: impl Into<String>) -> Client {
        Client {
            identity,
            run_time_id: run_time_id.into(),
            login: LoginApp::default(),
        }
    }

    /// 网络身份（同 crate 业务方法使用）。
    pub(crate) fn identity(&self) -> &Identity {
        &self.identity
    }

    /// 本进程 `runTimeId`。
    pub(crate) fn run_time_id(&self) -> &str {
        &self.run_time_id
    }

    /// 登录应用配置。
    pub(crate) fn login_app(&self) -> &LoginApp {
        &self.login
    }

    /// 执行一个端点：`GET https://{HOST}{path}`，返回原始响应
    /// （状态码由各业务方法判定，本层不代劳）。
    pub(crate) fn get<E: Endpoint>(&self, ep: &E) -> Result<Resp> {
        fetch(E::HOST, &ep.path(), E::TIMEOUT)
    }
}

/// 供后台 fire-and-forget 线程直接调用的原生传输（调用方持有已拼好的三元组）。
pub(crate) fn fetch(host: &str, path_query: &str, timeout: Timeout) -> Result<Resp> {
    match (host, 443u16).to_socket_addrs() {
        Ok(mut it) => {
            if it.next().is_none() {
                return Err(format!("网络异常，域名无法解析：{host}"));
            }
        }
        Err(e) => return Err(format!("网络异常，域名无法解析：{host}（{e}）")),
    };

    let ms = timeout.ms();
    // 每次新建内核（禁止复用）：连接池上限置 0，且内核随本次请求结束释放，
    // 等价 `FRESH_CONNECT + FORBID_REUSE`（服务端回 keep-alive 也主动关）。
    // SAFETY: 认证链自签，服务端证书无法通过系统根校验，关闭校验是登录必需。
    let inner = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_millis(ms))
        .pool_max_idle_per_host(0)
        // Cookie jar 默认禁用：不保存不回送任何 Cookie。
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .no_proxy()
        .build()
        .map_err(|e| format!("HTTP 客户端初始化失败: {e}"))?;

    let url = format!("https://{host}{path_query}");
    // 顺序即第 5 节要求的三行头顺序：Host / User-Agent / Accept。
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
    let req = inner
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
            crate::resp::extract_codekey(r.header_values("set-cookie")).unwrap(),
            "abc"
        );
    }
}
