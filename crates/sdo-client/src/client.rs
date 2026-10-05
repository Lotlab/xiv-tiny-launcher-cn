//! 最小 HTTP 传输层 + 会话客户端。
//!
//! - HTTP 客户端按超时种类缓存（连接不复用），固定 `Host / User-Agent / Accept` 三行头，
//!   不设 `Referer`，Cookie jar 禁用（只取 `CODEKEY`，见 [`crate::resp`]）。
//! - 成功条件（仅 `HTTP 200`；`200 + return_code != 0` 按失败走各自分支）由各业务方法判定，
//!   本层只透传状态码与原文。
//! - 错误分类见 [`Error`]；用户文案与排查细节分离，完整 URL 只写入日志。

use std::sync::OnceLock;
use std::time::Duration;

use proto::consts::{ACCEPT, TIMEOUT_AUTH_MS, TIMEOUT_DOWNLOAD_MS, UA};
use proto::log;

use crate::auxreq::Pending;
use crate::endpoint::{App, Endpoint, LOGIN_APP};

pub use crate::error::{Error, Result};

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
        serde_json::from_slice(&self.body).map_err(|e| Error::parse(format!("JSON 解析失败: {e}")))
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
/// （设备快照 + 本进程 `runTimeId` + 应用口径）。
///
/// 构造时克隆快照。应用口径默认是登录应用（[`LOGIN_APP`]），可用
/// [`Client::with_app`] 覆盖；换票的游戏应用口径按次传入
/// （[`Client::exchange`](crate::client::Client::exchange)）。
///
/// `Client` 同时持有自己发出的后台附属请求句柄，退出前必须调用
/// [`Client::wait_pending`]（见 [`crate::auxreq`]）。克隆共享同一份句柄清单。
#[derive(Debug, Clone)]
pub struct Client {
    identity: Identity,
    run_time_id: String,
    app: App,
    pub(crate) pending: Pending,
}

impl Client {
    /// 按冻结口径构造（登录应用 = [`LOGIN_APP`]）。
    pub fn new(identity: Identity, run_time_id: impl Into<String>) -> Client {
        Client::with_app(identity, run_time_id, LOGIN_APP)
    }

    /// 指定应用口径构造（需要非默认的登录应用时用）。
    pub fn with_app(
        identity: Identity,
        run_time_id: impl Into<String>,
        app: App,
    ) -> Client {
        Client {
            identity,
            run_time_id: run_time_id.into(),
            app,
            pending: Pending::default(),
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

    /// 应用口径。
    pub(crate) fn app(&self) -> &App {
        &self.app
    }

    /// 等待本客户端发出的后台附属请求结束，总预算 `budget`。
    ///
    /// 附属请求是后台线程，`process::exit` 会直接终止它们；退出前调用一次。
    pub fn wait_pending(&self, budget: Duration) {
        self.pending.wait(budget);
    }

    /// 执行一个端点：`GET https://{HOST}{path}`，返回原始响应
    /// （状态码由各业务方法判定，本层不代劳）。
    pub(crate) fn get<E: Endpoint>(&self, ep: &E) -> Result<Resp> {
        fetch(E::HOST, &ep.path(), E::TIMEOUT)
    }
}

/// HTTP 客户端：按超时种类各缓存一个。
///
/// 不每次请求新建：`reqwest::blocking::Client` 每构造一个都会创建一个线程运行 tokio
/// runtime，按每秒一次轮询会造成线程反复创建销毁。连接不复用由
/// `pool_max_idle_per_host(0)` 保证，缓存的只是这个线程与 TLS 配置。
fn http_client(timeout: Timeout) -> Result<&'static reqwest::blocking::Client> {
    static AUTH: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    static DOWNLOAD: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    let cell = match timeout {
        Timeout::Auth => &AUTH,
        Timeout::Download => &DOWNLOAD,
    };
    if let Some(c) = cell.get() {
        return Ok(c);
    }
    let built = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_millis(timeout.ms()))
        .pool_max_idle_per_host(0)
        .no_proxy()
        .build()
        .map_err(|e| Error::transport(format!("HTTP 客户端初始化失败: {e}")))?;
    // 并发首次调用时可能已有其他线程完成初始化；使用已有的，丢弃本次构造的。
    let _ = cell.set(built);
    cell.get()
        .ok_or_else(|| Error::transport("HTTP 客户端初始化失败"))
}

/// 用户可见的 URL：去掉 query（票据与设备指纹都在里面）。
fn display_url(host: &str, path_query: &str) -> String {
    match path_query.split_once('?') {
        Some((path, _)) => format!("https://{host}{path}"),
        None => format!("https://{host}{path_query}"),
    }
}

/// 底层传输函数：供后台线程直接调用，参数是已拼好的 host、path 与超时。
pub(crate) fn fetch(host: &str, path_query: &str, timeout: Timeout) -> Result<Resp> {
    let ms = timeout.ms();
    let inner = http_client(timeout)?;

    let url = format!("https://{host}{path_query}");
    let url_display = display_url(host, path_query);

    let mut self_headers = reqwest::header::HeaderMap::new();
    self_headers.insert(
        reqwest::header::HOST,
        reqwest::header::HeaderValue::from_str(host)
            .map_err(|e| Error::transport(format!("非法 Host 头: {e}")))?,
    );
    self_headers.insert(
        reqwest::header::USER_AGENT,
        reqwest::header::HeaderValue::from_str(UA)
            .map_err(|e| Error::transport(format!("非法 UA 头: {e}")))?,
    );
    self_headers.insert(
        reqwest::header::ACCEPT,
        reqwest::header::HeaderValue::from_str(ACCEPT)
            .map_err(|e| Error::transport(format!("非法 Accept 头: {e}")))?,
    );
    let req = inner
        .get(&url)
        .headers(self_headers)
        .timeout(Duration::from_millis(ms));

    let resp = req.send().map_err(|e| {
        // reqwest 的 Display 自带 URL，只写日志；用户可见的用去掉 query 的版本。
        log::debug(&format!("请求失败 {url}: {e}"));
        Error::transport(format!("请求失败 {url_display}: {}", e.without_url()))
    })?;
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
        .map_err(|e| {
            log::debug(&format!("读取响应体失败 {url}: {e}"));
            Error::transport(format!("读取响应体失败 {url_display}: {}", e.without_url()))
        })?
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

    /// 面向用户的 URL 不得带 query（票据/设备指纹都在里面）。
    #[test]
    fn display_url_drops_query() {
        assert_eq!(
            display_url("cas.sdo.com", "/authen/getGuid.json?codeKey=SECRET&guid=X"),
            "https://cas.sdo.com/authen/getGuid.json"
        );
        assert_eq!(
            display_url("cas.sdo.com", "/authen/getGuid.json"),
            "https://cas.sdo.com/authen/getGuid.json"
        );
        assert!(!display_url("h", "/p?a=1").contains("a=1"));
    }

    /// 真实验证：全部 host 都通过链 + hostname 严格校验
    /// （默认不跑，需网络：`cargo test -p sdo-client -- --ignored`）。
    #[test]
    #[ignore]
    fn tls_strict_verification_works_for_all_hosts() {
        let hosts = [
            proto::consts::HOST_CAS,
            proto::consts::HOST_N_CAS,
            proto::consts::HOST_GFC,
            proto::consts::HOST_UTILITY,
            proto::consts::HOST_V3LAUNCHER,
        ];
        let client = http_client(Timeout::Auth).unwrap();
        for host in hosts {
            let r = client.get(format!("https://{host}/")).send();
            // 状态码无所谓，握手通过即可。
            assert!(r.is_ok(), "{host} 未通过严格 TLS 校验: {:?}", r.err());
        }
    }
}
