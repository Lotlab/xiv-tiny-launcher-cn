//! CDN 网络层：`ver2.dat` 版本检查、`v3ctrl.xml` 鉴权、文件清单、文件下载。
//!
//! 与 `sdo-client`（登录协议）分开：CDN 需要长超时、流式落盘、断点续传，
//! 且证书策略可能不同，所以自带客户端。

pub mod auth;
pub mod filelist;
pub mod ver2;

use std::io::{Read, Write};
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
    Http { status: u16, url: String },
    Json(String),
    Auth(auth::AuthError),
    FileList(filelist::FileListError),
    Io(String),
    /// 下载内容校验失败。
    Verify(String),
    /// 鉴权过期（HTTP 458/403），调用方刷新后重试。
    AuthExpired { status: u16 },
}

impl std::fmt::Display for CdnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdnError::Client(e) => write!(f, "CDN 客户端初始化失败：{e}"),
            CdnError::Request(e) => write!(f, "CDN 请求失败：{e}"),
            CdnError::Http { status, url } => write!(f, "CDN 返回 HTTP {status}：{url}"),
            CdnError::Json(e) => write!(f, "CDN 响应解析失败：{e}"),
            CdnError::Auth(e) => write!(f, "{e}"),
            CdnError::FileList(e) => write!(f, "{e}"),
            CdnError::Io(e) => write!(f, "落盘失败：{e}"),
            CdnError::Verify(e) => write!(f, "下载校验失败：{e}"),
            CdnError::AuthExpired { status } => write!(f, "CDN 鉴权过期（HTTP {status}）"),
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
pub struct Cdn {
    client: Client,
}

/// 代理策略。缺省**直连**：CDN 对代理出口 IP 敏感，需要代理时显式指定。
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
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10))
            .danger_accept_invalid_certs(insecure)
            .user_agent("Mozilla/5.0");
        match proxy {
            ProxyMode::Env => {}
            ProxyMode::NoProxy => builder = builder.no_proxy(),
            ProxyMode::Explicit(p) => {
                let p =
                    reqwest::Proxy::all(p).map_err(|e| CdnError::Client(format!("代理非法：{e}")))?;
                builder = builder.proxy(p);
            }
        }
        let client = builder.build().map_err(|e| CdnError::Client(e.to_string()))?;
        Ok(Cdn { client })
    }

    fn get_bytes(&self, url: &str) -> Result<Vec<u8>, CdnError> {
        let resp = self
            .client
            .get(url)
            .send()
            .map_err(|e| CdnError::Request(format!("{url}：{}", e.without_url())))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(CdnError::Http {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| CdnError::Request(format!("{url}：读取响应体失败：{}", e.without_url())))
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
    pub fn get_authed(&self, auth: &auth::CdnAuth, url: &str) -> Result<Vec<u8>, CdnError> {
        let resp = self.authed_request(auth, url, 0)?.send().map_err(|e| {
            CdnError::Request(format!("{url}：{}", e.without_url()))
        })?;
        let status = resp.status();
        if status.as_u16() == 458 || status.as_u16() == 403 {
            return Err(CdnError::AuthExpired {
                status: status.as_u16(),
            });
        }
        if !status.is_success() {
            return Err(CdnError::Http {
                status: status.as_u16(),
                url: url.to_string(),
            });
        }
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| CdnError::Request(format!("{url}：读取响应体失败：{}", e.without_url())))
    }

    fn authed_request(
        &self,
        auth: &auth::CdnAuth,
        url: &str,
        resume_from: u64,
    ) -> Result<reqwest::blocking::RequestBuilder, CdnError> {
        let authed = auth.auth_url(url);
        let mut req = self.client.get(&authed);
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
            458 | 403 => {
                return Err(CdnError::AuthExpired {
                    status: status.as_u16(),
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
                .map_err(|e| CdnError::Request(format!("{url}：读取响应失败：{e}")))?;
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
}
