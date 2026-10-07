//! CDN 网络层：`ver2.dat` 版本检查、`v3ctrl.xml` 鉴权、文件清单、文件下载。
//!
//! 与 `sdo-client`（登录协议）分开：CDN 需要长超时、流式落盘、断点续传，
//! 且证书策略可能不同，所以自带客户端。

pub mod auth;
pub mod filelist;
pub mod httpdns;
pub mod ver2;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use md5::{Digest, Md5};
use reqwest::blocking::Client;

/// 国服游戏 id（与 `sdo-client` 区服表路径一致）。
pub const GAME_ID: &str = "100001900";
/// 当前 build id（与 `sdo-client` 区服表路径一致）。
///
/// 启动器实际传的是 `sdo_client::BUILD_ID`；这里只给示例/独立调用当默认值，
/// `launcher` 里有测试断言两者一致。
pub const BUILD_ID: &str = "8847";
/// 版本/清单 CDN host。
pub const HOST_V3LAUNCHER: &str = "v3launcher.jijiagames.com";
/// 鉴权配置 host。
pub const HOST_DOWNLOADER: &str = "downloader.dorado.sdo.com";

pub fn ver2_url(game_id: &str, build_id: &str) -> String {
    format!("https://{HOST_V3LAUNCHER}/v3launcher/build/ver2data/{game_id}/{build_id}/-1/ver2.dat")
}

pub fn v3ctrl_url(game_id: &str) -> String {
    format!("https://{HOST_DOWNLOADER}/v3launcher/{game_id}/v3ctrl.xml")
}

pub fn file_list_url(game_id: &str, build_id: &str) -> String {
    format!(
        "https://{HOST_V3LAUNCHER}/v3launcher/build/{game_id}/{build_id}/client-all-files-list/client_all_files_list.dat"
    )
}

#[derive(Debug)]
pub enum CdnError {
    Client(String),
    Request(String),
    Http {
        status: u16,
        url: String,
        /// 错误响应体长度（指纹：17≈公式错 / 11≈时间戳旧 / 0≈边缘拦截）。
        body_len: usize,
    },
    Json(String),
    Auth(auth::AuthError),
    FileList(filelist::FileListError),
    Io(String),
    /// 下载内容校验失败。
    Verify(String),
    /// CDN 配置失效（HTTP 458）：刷新鉴权后可重试。
    AuthExpired { status: u16 },
    /// 边缘节点拒绝（HTTP 403）：鉴权本身无错，换节点 + 退避后重试。
    EdgeRejected {
        status: u16,
        url: String,
        /// 含义同 `Http.body_len`（错误指纹）。
        body_len: usize,
    },
}

impl std::fmt::Display for CdnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdnError::Client(e) => write!(f, "CDN 客户端初始化失败：{e}"),
            CdnError::Request(e) => write!(f, "CDN 请求失败：{e}"),
            CdnError::Http { status, url, body_len } => {
                write!(f, "CDN 返回 HTTP {status}（响应体 {body_len} 字节）：{}", short_url(url))
            }
            CdnError::Json(e) => write!(f, "CDN 响应解析失败：{e}"),
            CdnError::Auth(e) => write!(f, "{e}"),
            CdnError::FileList(e) => write!(f, "{e}"),
            CdnError::Io(e) => write!(f, "落盘失败：{e}"),
            CdnError::Verify(e) => write!(f, "下载校验失败：{e}"),
            CdnError::AuthExpired { status } => write!(f, "CDN 配置失效（HTTP {status}），已刷新重试"),
            CdnError::EdgeRejected { status, url, body_len } => write!(
                f,
                "CDN 边缘节点拒绝（HTTP {status}，响应体 {body_len} 字节）：{}",
                short_url(url)
            ),
        }
    }
}

impl std::error::Error for CdnError {}

impl From<auth::AuthError> for CdnError {
    fn from(e: auth::AuthError) -> Self {
        CdnError::Auth(e)
    }
}

impl From<filelist::FileListError> for CdnError {
    fn from(e: filelist::FileListError) -> Self {
        CdnError::FileList(e)
    }
}

/// CDN 客户端（阻塞式）。
///
/// 底层的 HTTP 连接池是可重建的：边缘节点出问题时 [`Cdn::reconnect`] 会丢掉
/// 旧连接重建，新连接会重新 DNS 解析，有机会落到另一个边缘节点上；
/// [`Cdn::refresh_pins`] 更进一步，用 HTTPDNS 拿全量边缘 IP 并 pin 住轮换。
pub struct Cdn {
    client: std::cell::RefCell<Client>,
    insecure: bool,
    no_proxy: bool,
    proxy_url: Option<String>,
    /// host → HTTPDNS 边缘 IP（轮换顺序即连接优先级），重建连接时应用。
    /// 超过 [`PIN_TTL`] 未刷新的视为过期（边缘会变），重建时自动忽略。
    pins: std::cell::RefCell<HashMap<String, PinState>>,
}

/// pin 的有效期：超过这么久没刷新就不再 pin 住，避免边缘变更后还连旧 IP。
const PIN_TTL: Duration = Duration::from_secs(600);

/// 某 host 的 pin 状态：候选 IP + 已轮换次数（保证连续重试首选不同 IP）。
#[derive(Debug, Clone)]
struct PinState {
    ips: Vec<IpAddr>,
    seq: usize,
    at: std::time::Instant,
}

impl Default for PinState {
    fn default() -> Self {
        PinState {
            ips: Vec::new(),
            seq: 0,
            at: std::time::Instant::now(),
        }
    }
}

/// pin 是否仍在有效期内。
fn pin_fresh(at: std::time::Instant) -> bool {
    at.elapsed() <= PIN_TTL
}

/// 把 URL 的 host 换成备用 host（scheme/path/query 保持不变）。
///
/// 鉴权 hash 只覆盖 path，换 host 后鉴权仍然有效；解析失败时原样返回。
pub fn swap_host(url: &str, new_host: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            if u.set_host(Some(new_host)).is_err() {
                return url.to_string();
            }
            u.to_string()
        }
        Err(_) => url.to_string(),
    }
}

/// 读最多 `limit` 字节，只为记录错误指纹长度（不关心内容）。
fn fingerprint_len(body: impl std::io::Read, limit: u64) -> usize {
    use std::io::Read;
    let mut buf = Vec::new();
    let _ = body.take(limit).read_to_end(&mut buf);
    buf.len()
}

/// 终端/日志用的短 URL：`host + 路径尾部`。
///
/// 完整鉴权 URL 含一次性 hash（半小时级过期），打全了既刷屏又没法复用；
/// 路径尾部（文件 ID / 清单名）已足以定位文件。
fn short_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("");
            let path = u.path();
            if path.chars().count() > 40 {
                let tail: String = path.chars().rev().take(32).collect();
                let tail: String = tail.chars().rev().collect();
                format!("{host}/…{tail}")
            } else {
                format!("{host}{path}")
            }
        }
        Err(_) => url.chars().take(80).collect(),
    }
}

/// 请求错误：短 URL + 去 URL 的底层错误（终端一行能放下）。
fn req_err(url: &str, detail: impl std::fmt::Display) -> CdnError {
    CdnError::Request(format!("{}：{detail}", short_url(url)))
}

/// 代理策略：跟随环境变量（reqwest 默认）、显式直连，或指定代理。
#[derive(Debug, Clone, Copy)]
pub enum ProxyMode<'a> {
    /// 跟随环境变量（`http_proxy` / `https_proxy`）。
    Env,
    /// 不走任何代理。
    NoProxy,
    /// 显式指定代理。
    Explicit(&'a str),
}

impl Cdn {
    /// `insecure=true` 跳过证书校验；`proxy` 决定代理策略。
    pub fn with_options(insecure: bool, proxy: ProxyMode<'_>) -> Result<Cdn, CdnError> {
        let (no_proxy, proxy_url) = match proxy {
            ProxyMode::Env => (false, None),
            ProxyMode::NoProxy => (true, None),
            ProxyMode::Explicit(p) => (false, Some(p.to_string())),
        };
        let client = Self::build_client(
            insecure,
            no_proxy,
            proxy_url.as_deref(),
            &HashMap::new(),
        )?;
        Ok(Cdn {
            client: std::cell::RefCell::new(client),
            insecure,
            no_proxy,
            proxy_url,
            pins: std::cell::RefCell::new(HashMap::new()),
        })
    }

    fn build_client(
        insecure: bool,
        no_proxy: bool,
        proxy_url: Option<&str>,
        pins: &HashMap<String, PinState>,
    ) -> Result<Client, CdnError> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure)
            .user_agent("Mozilla/5.0");
        if no_proxy {
            builder = builder.no_proxy();
        }
        if let Some(p) = proxy_url {
            let p =
                reqwest::Proxy::all(p).map_err(|e| CdnError::Client(format!("代理非法：{e}")))?;
            builder = builder.proxy(p);
        }
        // 把 HTTPDNS 的边缘 IP pin 到对应域名（等价 curl `CURLOPT_RESOLVE`；
        // 端口 0 表示按 scheme 取常规端口，SNI/证书校验保持域名不变）。
        // 过期 pin 自动忽略（边缘 IP 会变），下次重试时由 refresh_pins 重新拿。
        for (host, pin) in pins {
            if pin.ips.is_empty() || !pin_fresh(pin.at) {
                continue;
            }
            let addrs: Vec<SocketAddr> =
                pin.ips.iter().map(|ip| SocketAddr::new(*ip, 0)).collect();
            builder = builder.resolve_to_addrs(host, &addrs);
        }
        builder.build().map_err(|e| CdnError::Client(e.to_string()))
    }

    fn rebuild(&self) -> Result<(), CdnError> {
        let fresh = Self::build_client(
            self.insecure,
            self.no_proxy,
            self.proxy_url.as_deref(),
            &self.pins.borrow(),
        )?;
        *self.client.borrow_mut() = fresh;
        Ok(())
    }

    /// 重建底层连接（丢掉连接池）：重试前调用，新连接会重新 DNS 解析，
    /// GSLB 轮换下有机会落到另一个边缘节点，绕开出问题的节点。
    pub fn reconnect(&self) -> Result<(), CdnError> {
        self.rebuild()
    }

    /// 用 HTTPDNS 刷新 `host` 的边缘 IP（轮换首选），pin 住并重建连接。
    ///
    /// 成功返回候选 IP 数；DoH 查不到时返回 Err，调用方回退到普通 [`Cdn::reconnect`]。
    /// 只在重试路径调用，健康时零额外开销。
    pub fn refresh_pins(&self, host: &str) -> Result<usize, CdnError> {
        let ips = httpdns::resolve_host(host).map_err(CdnError::Request)?;
        if ips.is_empty() {
            return Err(CdnError::Request(format!("HTTPDNS 无 {host} 的边缘 IP")));
        }
        let count = ips.len();
        let mut pins = self.pins.borrow_mut();
        let pin = pins.entry(host.to_string()).or_default();
        pin.seq = pin.seq.wrapping_add(1);
        // 轮换首位：连续重试每次首选不同边缘 IP。
        pin.ips = httpdns::rotated(&ips, pin.seq);
        pin.at = std::time::Instant::now();
        drop(pins);
        self.rebuild()?;
        Ok(count)
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, CdnError> {
        let resp = self
            .client
            .borrow()
            .get(url)
            .send()
            .map_err(|e| req_err(url, e.without_url()))?;
        let status = resp.status();
        if !status.is_success() {
            let body_len = fingerprint_len(resp, 4096);
            return Err(CdnError::Http {
                status: status.as_u16(),
                url: url.to_string(),
                body_len,
            });
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| req_err(url, format!("读取响应体失败：{}", e.without_url())))
    }

    /// 拉取并解析 `ver2.dat`。
    pub fn fetch_ver2(&self, game_id: &str, build_id: &str) -> Result<ver2::Ver2, CdnError> {
        let url = ver2_url(game_id, build_id);
        let bytes = self.get_bytes(&url)?;
        serde_json::from_slice(&bytes).map_err(|e| CdnError::Json(e.to_string()))
    }

    /// 拉取并解析 `v3ctrl.xml`，返回**未解密**的字段。
    pub fn fetch_v3ctrl_raw(&self, game_id: &str) -> Result<auth::RawV3Ctrl, CdnError> {
        let url = v3ctrl_url(game_id);
        let bytes = self.get_bytes(&url)?;
        auth::parse_v3ctrl(&bytes, game_id).map_err(CdnError::Auth)
    }

    /// 拉取 `v3ctrl.xml` 并解出鉴权材料。
    pub fn fetch_auth(&self, game_id: &str) -> Result<auth::CdnAuth, CdnError> {
        let raw = self.fetch_v3ctrl_raw(game_id)?;
        auth::CdnAuth::from_raw_with_key(raw, crate::keys::CDN_RSA_PUBLIC_KEY_PEM)
            .map_err(CdnError::Auth)
    }

    /// 拉取并解析全量文件清单。
    pub fn fetch_file_list(&self, game_id: &str, build_id: &str) -> Result<filelist::FileList, CdnError> {
        let url = file_list_url(game_id, build_id);
        let bytes = self.get_bytes(&url)?;
        filelist::parse_file_list(&String::from_utf8_lossy(&bytes)).map_err(CdnError::FileList)
    }

    /// 带鉴权拉一个小文件（走 `auth_url` + 头）。
    ///
    /// 只有 458（配置失效）可刷新重试；403 是边缘节点拒绝，鉴权本身无错，
    /// 调用方应换节点 + 退避重试（见 `download::Downloader`）。
    pub fn get_authed(&self, auth: &auth::CdnAuth, url: &str) -> Result<Vec<u8>, CdnError> {
        let resp = self.authed_request(auth, url, 0)?.send().map_err(|e| req_err(url, e.without_url()))?;
        let status = resp.status();
        if status.as_u16() == 458 {
            return Err(CdnError::AuthExpired {
                status: status.as_u16(),
            });
        }
        if status.as_u16() == 403 {
            return Err(CdnError::EdgeRejected {
                status: status.as_u16(),
                url: url.to_string(),
                body_len: fingerprint_len(resp, 4096),
            });
        }
        if !status.is_success() {
            return Err(CdnError::Http {
                status: status.as_u16(),
                url: url.to_string(),
                body_len: fingerprint_len(resp, 4096),
            });
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| req_err(url, format!("读取响应体失败：{}", e.without_url())))
    }

    fn authed_request(
        &self,
        auth: &auth::CdnAuth,
        url: &str,
        resume_from: u64,
    ) -> Result<reqwest::blocking::RequestBuilder, CdnError> {
        let authed = auth.auth_url(url);
        let mut req = self.client.borrow().get(&authed);
        for (k, v) in auth.headers() {
            req = req.header(k, v);
        }
        if resume_from > 0 {
            req = req.header("Range", format!("bytes={resume_from}-"));
        }
        Ok(req)
    }

    /// 下载单个文件到 `dest`，支持断点续传与 size/MD5 校验。
    ///
    /// 返回实际写入的字节数（含续传部分）。校验失败会删除目标文件。
    pub fn download_file(
        &self,
        auth: &auth::CdnAuth,
        url: &str,
        dest: &Path,
        expected_size: u64,
        expected_hash: &str,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<u64, CdnError> {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| CdnError::Io(e.to_string()))?;
        }

        let existing = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        let mut resume_from = existing;
        if expected_size != 0 && existing > expected_size {
            // 超过目标大小：当作坏文件重下。
            let _ = std::fs::remove_file(dest);
            resume_from = 0;
        }

        let resp = self
            .authed_request(auth, url, resume_from)?
            .send()
            .map_err(|e| CdnError::Request(format!("{url}：{}", e.without_url())))?;
        let status = resp.status();
        match status.as_u16() {
            458 => {
                return Err(CdnError::AuthExpired {
                    status: status.as_u16(),
                })
            }
            403 => {
                return Err(CdnError::EdgeRejected {
                    status: status.as_u16(),
                    url: url.to_string(),
                    body_len: fingerprint_len(resp, 4096),
                })
            }
            416 => {
                // Range 不可满足：已存在的部分文件非法，删掉重来。
                let _ = std::fs::remove_file(dest);
                return Err(CdnError::Verify("Range 不可满足，已删除本地残片".into()));
            }
            _ if !status.is_success() => {
                return Err(CdnError::Http {
                    status: status.as_u16(),
                    url: url.to_string(),
                    body_len: fingerprint_len(resp, 4096),
                })
            }
            _ => {}
        }

        let resuming = resume_from > 0 && status.as_u16() == 206;
        if resume_from > 0 && !resuming {
            // 服务端不支持 Range：从头写。
            resume_from = 0;
        }

        let mut hasher = Md5::new();
        if resuming {
            hash_existing(dest, &mut hasher)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(resuming)
            .truncate(!resuming)
            .open(dest)
            .map_err(|e| CdnError::Io(format!("{}：{e}", dest.display())))?;

        let mut total = resume_from;
        let mut resp = resp;
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = resp
                .read(&mut buf)
                .map_err(|e| req_err(url, format!("读取响应失败：{e}")))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])
                .map_err(|e| CdnError::Io(e.to_string()))?;
            total += n as u64;
            on_progress(total);
        }
        file.flush().map_err(|e| CdnError::Io(e.to_string()))?;
        drop(file);

        if expected_size != 0 && total != expected_size {
            let _ = std::fs::remove_file(dest);
            return Err(CdnError::Verify(format!(
                "大小不符：期望 {expected_size}，实际 {total}"
            )));
        }
        let got = crate::hash::hex_upper(&hasher.finalize());
        if !expected_hash.is_empty() && !got.eq_ignore_ascii_case(expected_hash) {
            let _ = std::fs::remove_file(dest);
            return Err(CdnError::Verify(format!(
                "MD5 不符：期望 {expected_hash}，实际 {got}"
            )));
        }
        Ok(total)
    }
}

/// 把已存在文件流式喂进 hasher（用于续传时补齐前半段摘要）。
fn hash_existing(path: &Path, hasher: &mut Md5) -> Result<(), CdnError> {
    let mut f = std::fs::File::open(path).map_err(|e| CdnError::Io(e.to_string()))?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| CdnError::Io(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(())
}

/// 校验本地文件是否与清单条目一致（size + MD5）。
pub fn verify_local(path: &Path, entry: &filelist::FileEntry) -> bool {
    verify_file(path, entry.size, &entry.hash)
}

/// 校验本地文件是否等于给定 size + MD5（size=0 表示不校验大小，md5 为空表示不校验 MD5）。
pub fn verify_file(path: &Path, size: u64, hash: &str) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if size != 0 && meta.len() != size {
        return false;
    }
    if hash.is_empty() {
        return true;
    }
    let mut hasher = Md5::new();
    if hash_existing(path, &mut hasher).is_err() {
        return false;
    }
    crate::hash::hex_upper(&hasher.finalize()).eq_ignore_ascii_case(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_shape() {
        assert_eq!(
            ver2_url("100001900", "8847"),
            "https://v3launcher.jijiagames.com/v3launcher/build/ver2data/100001900/8847/-1/ver2.dat"
        );
        assert_eq!(
            v3ctrl_url("100001900"),
            "https://downloader.dorado.sdo.com/v3launcher/100001900/v3ctrl.xml"
        );
        assert_eq!(
            file_list_url("100001900", "8847"),
            "https://v3launcher.jijiagames.com/v3launcher/build/100001900/8847/client-all-files-list/client_all_files_list.dat"
        );
    }

    #[test]
    fn verify_local_checks_size_and_md5() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.bin");
        std::fs::write(&p, b"hello").unwrap();
        let good = filelist::FileEntry {
            path: "a.bin".into(),
            size: 5,
            hash: crate::hash::md5_hex_upper(b"hello"),
        };
        assert!(verify_local(&p, &good));
        assert!(!verify_local(&p, &filelist::FileEntry { size: 6, ..good.clone() }));
        assert!(!verify_local(&p, &filelist::FileEntry { hash: "00".into(), ..good }));
    }

    #[test]
    fn swap_host_keeps_scheme_path_query() {
        assert_eq!(
            swap_host("https://a.example/x/y?z=1", "b.example"),
            "https://b.example/x/y?z=1"
        );
        // 非法输入原样返回，不 panic。
        assert_eq!(swap_host("not a url", "b.example"), "not a url");
        assert_eq!(swap_host("https://a.example/x", "not a host!!"), "https://a.example/x");
    }

    #[test]
    fn stale_pins_expire() {
        assert!(pin_fresh(std::time::Instant::now()));
        let old = std::time::Instant::now()
            .checked_sub(PIN_TTL + Duration::from_secs(1))
            .unwrap();
        assert!(!pin_fresh(old));
    }

    #[test]
    fn short_url_keeps_host_and_tail() {
        // 长路径只留尾部（文件 ID），终端一行能放下。
        assert_eq!(
            short_url("https://ff14.jijiagames.com/v3client/build/100001900/8847/apppc/1131/game/1113767C0B923103902456D56C73798B"),
            "ff14.jijiagames.com/…1113767C0B923103902456D56C73798B"
        );
        // 短路径原样保留 host + path。
        assert_eq!(short_url("https://h/a/b.dat"), "h/a/b.dat");
    }
}
