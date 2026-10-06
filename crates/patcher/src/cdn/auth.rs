//! CDN 鉴权：`v3ctrl.xml` → RSA 公钥解密 → 鉴权 URL / 请求头。
//!
//! `v3ctrl.xml`（GBK）：
//!
//! ```xml
//! <project>
//!   <game gameid="100001900">
//!     <md5key>…</md5key>                     <!-- Referer -->
//!     <client type="1"><md5key>…</md5key></client>
//!     <server-let type="0"><md5key>…</md5key></server-let>
//!     <referer><value1>…</value1><value2>…</value2></referer>  <!-- User-Agent -->
//!     <iplist type="2">…</iplist>            <!-- 鉴权方案 0/1/2 -->
//!   </game>
//! </project>
//! ```
//!
//! 这些 `<md5key>` / `<valueN>` 都是 RSA **公钥加密**后的 hex；用打包注入的公钥
//! 做公钥运算 + PKCS#1 v1.5 type-1 去填充，得到明文字符串。
//!
//! `cdn_token = client[..16] + server-let[-16..]`，再按 `iplist@type` 把
//! `hash / time` 前缀插进 URL（见 [`CdnAuth::auth_url`]）。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::events::Event;
use quick_xml::Reader;
use rsa::pkcs8::DecodePublicKey;
use rsa::traits::PublicKeyParts;

use crate::hash;

#[derive(Debug)]
pub enum AuthError {
    Xml(String),
    /// `v3ctrl.xml` 里没有这个 game。
    GameNotFound(String),
    /// 缺字段。
    Missing(&'static str),
    Hex(String),
    Rsa(String),
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::Xml(e) => write!(f, "v3ctrl.xml 解析失败：{e}"),
            AuthError::GameNotFound(id) => write!(f, "v3ctrl.xml 里没有 gameid={id}"),
            AuthError::Missing(k) => write!(f, "v3ctrl.xml 缺少 {k}"),
            AuthError::Hex(e) => write!(f, "v3ctrl.xml 的 hex 字段非法：{e}"),
            AuthError::Rsa(e) => write!(f, "RSA 公钥解密失败：{e}"),
        }
    }
}

impl std::error::Error for AuthError {}

/// `v3ctrl.xml` 里与鉴权相关的原始（仍加密的）字段。
#[derive(Debug, Clone, Default)]
pub struct RawV3Ctrl {
    pub project_md5_key: String,
    pub client_md5_key: String,
    pub server_let_md5_key: String,
    pub referer_value: String,
    pub config_flag: u32,
}

/// 解出后的鉴权材料。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnAuth {
    /// 作为 `Referer` 头发送。
    pub project_md5_key: String,
    /// 作为 `User-Agent` 头发送。
    pub referer_value: String,
    /// `client[..16] + server-let[-16..]`。
    pub cdn_token: String,
    /// `iplist@type`。
    pub config_flag: u32,
}

impl CdnAuth {
    /// 用给定 RSA 公钥解密 `raw`。
    pub fn from_raw_with_key(raw: RawV3Ctrl, rsa_pem: &str) -> Result<CdnAuth, AuthError> {
        if raw.project_md5_key.is_empty() {
            return Err(AuthError::Missing("game/md5key"));
        }
        if raw.client_md5_key.is_empty() {
            return Err(AuthError::Missing("game/client/md5key"));
        }
        if raw.server_let_md5_key.is_empty() {
            return Err(AuthError::Missing("game/server-let/md5key"));
        }
        let project_md5_key = rsa_public_decrypt(rsa_pem, &raw.project_md5_key)?;
        let client = rsa_public_decrypt(rsa_pem, &raw.client_md5_key)?;
        let server_let = rsa_public_decrypt(rsa_pem, &raw.server_let_md5_key)?;
        let referer_value = if raw.referer_value.is_empty() {
            String::new()
        } else {
            rsa_public_decrypt(rsa_pem, &raw.referer_value)?
        };

        // 与参考实现一致：client 前 16 字符 + server-let 后 16 字符。
        let head = &client[..client.len().min(16)];
        let tail = &server_let[server_let.len().saturating_sub(16)..];
        let cdn_token = format!("{head}{tail}");

        Ok(CdnAuth {
            project_md5_key,
            referer_value,
            cdn_token,
            config_flag: raw.config_flag,
        })
    }

    /// 按鉴权方案给 URL 加 `hash` / `time` 前缀。
    ///
    /// - `flag 0`：原样返回。
    /// - `flag 1`：`{scheme}://{host}/{MD5(token+time+path)}{path}`，`time` 为 UTC+8 的
    ///   `YYYYMMDDHHMM`（只参与 hash，**不进 URL**）。
    /// - `flag 2`：`{scheme}://{host}/{MD5(token+path+time_hex)}/{time_hex}{path}`。
    pub fn auth_url(&self, input_url: &str) -> String {
        self.auth_url_at(input_url, unix_now())
    }

    /// 同上，但显式给时间戳（便于测试）。
    ///
    /// 时间戳超出 `time` 的可表示范围时原样返回 `input_url`（不 panic；这种
    /// URL 到服务端也是 403，不会静默放过）。
    pub fn auth_url_at(&self, input_url: &str, unix: i64) -> String {
        if self.config_flag == 0 {
            return input_url.to_string();
        }
        let url = match url::Url::parse(input_url) {
            Ok(u) => u,
            Err(_) => return input_url.to_string(),
        };
        let base = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
        let path = url.path();
        let query = match url.query() {
            Some(q) if !q.is_empty() => format!("?{q}"),
            _ => String::new(),
        };

        match self.config_flag {
            1 => {
                let Some(t) = ymdhm_utc8(unix) else {
                    return input_url.to_string();
                };
                let hash = hash::md5_hex_upper(format!("{}{}{}", self.cdn_token, t, path).as_bytes());
                format!("{base}/{hash}{path}{query}")
            }
            2 => {
                let t = format!("{unix:x}");
                let hash =
                    hash::md5_hex_upper(format!("{}{}{}", self.cdn_token, path, t).as_bytes());
                format!("{base}/{hash}/{t}{path}{query}")
            }
            _ => input_url.to_string(),
        }
    }

    /// 请求头：`User-Agent`（referer 值）+ `Referer`（project key）。
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        vec![
            ("User-Agent", self.referer_value.clone()),
            ("Referer", self.project_md5_key.clone()),
        ]
    }
}

/// 解析 `v3ctrl.xml`（GBK 字节），取 `game_id` 那一份。
pub fn parse_v3ctrl(bytes: &[u8], game_id: &str) -> Result<RawV3Ctrl, AuthError> {
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(true);

    let mut path: Vec<String> = Vec::new();
    let mut current_game = String::new();
    let mut in_target = false;
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut iplist_type = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "game" {
                    current_game = attr(&e, "gameid").unwrap_or_default();
                    in_target = current_game == game_id;
                    path.clear();
                }
                if name == "iplist" && in_target {
                    iplist_type = attr(&e, "type").unwrap_or_default();
                }
                path.push(name);
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name == "iplist" && in_target {
                    iplist_type = attr(&e, "type").unwrap_or_default();
                }
            }
            Ok(Event::End(_)) => {
                path.pop();
            }
            Ok(Event::Text(t)) => {
                if in_target && !path.is_empty() {
                    let key = path.join("/");
                    let value = t.unescape().map(|c| c.into_owned()).unwrap_or_default();
                    fields.insert(key, value);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(AuthError::Xml(e.to_string())),
        }
    }

    if !fields.keys().any(|k| k.starts_with("game/")) && current_game != game_id {
        return Err(AuthError::GameNotFound(game_id.to_string()));
    }

    Ok(RawV3Ctrl {
        project_md5_key: fields.get("game/md5key").cloned().unwrap_or_default(),
        client_md5_key: fields.get("game/client/md5key").cloned().unwrap_or_default(),
        server_let_md5_key: fields
            .get("game/server-let/md5key")
            .cloned()
            .unwrap_or_default(),
        referer_value: fields
            .get("game/referer/value2")
            .or_else(|| fields.get("game/referer/value1"))
            .cloned()
            .unwrap_or_default(),
        config_flag: iplist_type.parse().unwrap_or(0),
    })
}

fn attr(e: &quick_xml::events::BytesStart<'_>, name: &str) -> Option<String> {
    for a in e.attributes().flatten() {
        if a.key.as_ref() == name.as_bytes() {
            return a.unescape_value().ok().map(|v| v.into_owned());
        }
    }
    None
}

/// RSA 公钥运算 + PKCS#1 v1.5 type-1 去填充（等价 OpenSSL `public_decrypt(PKCS1)`）。
pub fn rsa_public_decrypt(pem: &str, encrypted_hex: &str) -> Result<String, AuthError> {
    if encrypted_hex.is_empty() {
        return Ok(String::new());
    }
    let bytes = hex_decode(encrypted_hex)?;
    let key = rsa::RsaPublicKey::from_public_key_pem(pem).map_err(|e| AuthError::Rsa(e.to_string()))?;
    let c = rsa::BigUint::from_bytes_be(&bytes);
    let m = c.modpow(key.e(), key.n());
    let k = key.n().bits().div_ceil(8);
    let mut buf = m.to_bytes_be();
    while buf.len() < k {
        buf.insert(0, 0);
    }
    if buf.len() < 11 || buf[0] != 0 || buf[1] != 1 {
        return Err(AuthError::Rsa("填充不是 PKCS#1 type-1".into()));
    }
    let sep = buf[2..]
        .iter()
        .position(|&b| b == 0)
        .map(|i| i + 2)
        .ok_or_else(|| AuthError::Rsa("填充缺少分隔 0x00".into()))?;
    Ok(String::from_utf8_lossy(&buf[sep + 1..]).into_owned())
}

fn hex_decode(s: &str) -> Result<Vec<u8>, AuthError> {
    hash::decode_hex(s).ok_or_else(|| AuthError::Hex(format!("非法 hex：{s}")))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `unix` 时刻在 UTC+8 的 `YYYYMMDDHHMM`；超出可表示范围返回 `None`。
fn ymdhm_utc8(unix: i64) -> Option<String> {
    let offset = time::UtcOffset::from_hms(8, 0, 0).expect("UTC+8 是合法偏移");
    let t = time::OffsetDateTime::from_unix_timestamp(unix).ok()?.to_offset(offset);
    Some(format!(
        "{:04}{:02}{:02}{:02}{:02}",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_game_fields_and_iplist() {
        let xml = r#"<?xml version="1.0" encoding="gbk"?>
<project>
  <game gameid="100001900">
    <md5key>AABB</md5key>
    <server type="1"><md5key>11</md5key></server>
    <client type="1"><md5key>CCDD</md5key></client>
    <server-let type="0"><md5key>EEFF</md5key></server-let>
    <referer><value1>01</value1><value2>02</value2><value3>03</value3></referer>
    <iplist type="2"><value1>x</value1></iplist>
  </game>
  <game gameid="791000827"><md5key>9999</md5key><iplist type="1"/></game>
</project>"#;
        let raw = parse_v3ctrl(xml.as_bytes(), "100001900").unwrap();
        assert_eq!(raw.project_md5_key, "AABB");
        assert_eq!(raw.client_md5_key, "CCDD");
        assert_eq!(raw.server_let_md5_key, "EEFF");
        assert_eq!(raw.referer_value, "02");
        assert_eq!(raw.config_flag, 2);
    }

    #[test]
    fn missing_game_errors() {
        let xml = r#"<project><game gameid="1"><md5key>A</md5key></game></project>"#;
        assert!(matches!(
            parse_v3ctrl(xml.as_bytes(), "100001900"),
            Err(AuthError::GameNotFound(_))
        ));
    }

    #[test]
    fn auth_url_flag2_matches_reference_formula() {
        let auth = CdnAuth {
            project_md5_key: "P".into(),
            referer_value: "UA".into(),
            cdn_token: "EKUWRI5KXXAIDlQ0mBNLa7XkjU1JNFuL".into(),
            config_flag: 2,
        };
        // 固定时间戳，与参考实现公式对齐：hash = MD5(token + path + hex(ts))
        let ts = 0x18d2_0000_i64;
        let path = "/a/b.dat";
        let url = format!("https://h{path}");
        let got = auth.auth_url_at(&url, ts);
        let expect_hash = hash::md5_hex_upper(format!("{}{}{:x}", auth.cdn_token, path, ts).as_bytes());
        assert_eq!(got, format!("https://h/{expect_hash}/{:x}{path}", ts));
    }

    #[test]
    fn auth_url_flag0_is_identity() {
        let auth = CdnAuth {
            project_md5_key: String::new(),
            referer_value: String::new(),
            cdn_token: String::new(),
            config_flag: 0,
        };
        assert_eq!(auth.auth_url("https://h/x"), "https://h/x");
    }

    #[test]
    fn ymdhm_is_utc_plus_8() {
        // 1970-01-01T00:00:00Z → UTC+8 是当天 08:00
        assert_eq!(ymdhm_utc8(0).as_deref(), Some("197001010800"));
        // 2026-09-14T16:00:00Z == 1789401600 → UTC+8 是 2026-09-15 00:00
        assert_eq!(ymdhm_utc8(1_789_401_600).as_deref(), Some("202609150000"));
        // 越界时间戳不 panic。
        assert_eq!(ymdhm_utc8(i64::MAX), None);
        assert_eq!(
            auth_url_at_flag1(i64::MAX),
            "https://h/a/b.dat",
            "时间戳非法时原样返回"
        );
    }

    fn auth_url_at_flag1(unix: i64) -> String {
        CdnAuth {
            project_md5_key: "P".into(),
            referer_value: "UA".into(),
            cdn_token: "T".into(),
            config_flag: 1,
        }
        .auth_url_at("https://h/a/b.dat", unix)
    }

    /// flag 1：`hash = MD5(token + UTC+8 的 YYYYMMDDHHMM + path)`，时间不进 URL。
    /// 期望 URL 与参考实现 `ffxiv-automatic-data-fetcher` 的 `build_cdn_auth_token` 一致。
    #[test]
    fn auth_url_flag1_matches_reference_formula() {
        let auth = CdnAuth {
            project_md5_key: "P".into(),
            referer_value: "UA".into(),
            cdn_token: "EKUWRI5KXXAIDlQ0mBNLa7XkjU1JNFuL".into(),
            config_flag: 1,
        };
        // 0x18d20000 → UTC+8 为 1983-03-13 23:02，hash 外部算好写死。
        assert_eq!(
            auth.auth_url_at("https://h/a/b.dat", 0x18d2_0000),
            "https://h/B5C83AAA84ED1AFB98EB457084B18D63/a/b.dat"
        );
    }
}
