//! 本地版本元数据 `LocalVersion3.xml` 的编解码。
//!
//! 磁盘格式（逆向自 `Launcher.dat`，见 `crypto::des`）：
//!
//! ```text
//! [4B magic = FF FF FF FF][3DES-ECB 密文，长度 8 的倍数][4B 小端明文长度]
//! ```
//!
//! 明文是 XML 包一段 JSON：
//!
//! ```text
//! <?xmlversion="1.0"encoding="utf-8"?><Root><zone{game}_{build}_v3>{
//!    "product_name" : "zone{game}_{build}_v3",
//!    "version" : { "v" : "<internal>", "view" : "<display>" }
//! }
//! </zone{game}_{build}_v3></Root>
//! ```

use crate::crypto::des;
use crate::keys;

/// 文件头魔数（小端 `FF FF FF FF`）。
const MAGIC: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaError {
    /// 太短（不足 8 字节）。
    TooShort,
    /// 头 4 字节不是 `FF FF FF FF`。
    BadMagic,
    /// 密文长度不是 8 的倍数。
    NotAligned,
    /// 长度字段超出明文范围。
    BadLength,
    /// 明文不是预期的 XML/JSON。
    BadContent(String),
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::TooShort => write!(f, "版本元数据文件太短"),
            MetaError::BadMagic => write!(f, "版本元数据文件头不是 FF FF FF FF"),
            MetaError::NotAligned => write!(f, "版本元数据密文长度不是 8 的倍数"),
            MetaError::BadLength => write!(f, "版本元数据长度字段非法"),
            MetaError::BadContent(e) => write!(f, "版本元数据内容非法：{e}"),
        }
    }
}

impl std::error::Error for MetaError {}

/// 本地版本信息（`product_name` + `version.v` / `version.view`）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VersionMeta {
    pub product_name: String,
    pub version: VersionEntry,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VersionEntry {
    /// internal 版本，如 `0.0.0.29`。
    pub v: String,
    /// display 版本，如 `2026.09.15.0000.0000_7.56`。
    pub view: String,
}

impl VersionMeta {
    /// `zone{game_id}_{build_id}_v3`。
    pub fn product(game_id: &str, build_id: &str) -> String {
        format!("zone{game_id}_{build_id}_v3")
    }

    pub fn new(game_id: &str, build_id: &str, internal: &str, display: &str) -> VersionMeta {
        VersionMeta {
            product_name: Self::product(game_id, build_id),
            version: VersionEntry {
                v: internal.to_string(),
                view: display.to_string(),
            },
        }
    }
}

/// 用给定密钥解出明文。
fn decode_with_key(key: &[u8; 16], raw: &[u8]) -> Result<Vec<u8>, MetaError> {
    if raw.len() < 8 {
        return Err(MetaError::TooShort);
    }
    if raw[..4] != MAGIC {
        return Err(MetaError::BadMagic);
    }
    let ct = &raw[4..raw.len() - 4];
    if !ct.len().is_multiple_of(8) {
        return Err(MetaError::NotAligned);
    }
    let plain = des::des3_ecb(key, ct, true);
    let len = u32::from_le_bytes([raw[raw.len() - 4], raw[raw.len() - 3], raw[raw.len() - 2], raw[raw.len() - 1]]) as usize;
    if len > plain.len() {
        return Err(MetaError::BadLength);
    }
    Ok(plain[..len].to_vec())
}

/// 用给定密钥加密（零填充到 8 的倍数）。
fn encode_with_key(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let mut padded = plain.to_vec();
    while !padded.len().is_multiple_of(8) {
        padded.push(0);
    }
    let ct = des::des3_ecb(key, &padded, false);
    let mut out = Vec::with_capacity(ct.len() + 8);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&ct);
    out.extend_from_slice(&(plain.len() as u32).to_le_bytes());
    out
}

/// 解出明文并解析成 [`VersionMeta`]（密钥由打包时注入）。
pub fn parse(raw: &[u8]) -> Result<VersionMeta, MetaError> {
    let plain = decode_with_key(&keys::meta_des_key(), raw)?;
    parse_plain(&String::from_utf8_lossy(&plain))
}

/// 把 [`VersionMeta`] 序列化成官方那种「XML 包 JSON」的明文。
fn to_plain(meta: &VersionMeta) -> Vec<u8> {
    let zone = &meta.product_name;
    let mut s = String::new();
    s.push_str("<?xmlversion=\"1.0\"encoding=\"utf-8\"?><Root><");
    s.push_str(zone);
    s.push_str(">{\n");
    s.push_str(&format!("   \"product_name\" : \"{}\",\n", zone));
    s.push_str("   \"version\" : {\n");
    s.push_str(&format!("      \"v\" : \"{}\",\n", meta.version.v));
    s.push_str(&format!("      \"view\" : \"{}\"\n", meta.version.view));
    s.push_str("   }\n}\n</");
    s.push_str(zone);
    s.push_str("></Root>");
    s.into_bytes()
}

/// 序列化并加密成磁盘格式（密钥由打包时注入）。
pub fn build(meta: &VersionMeta) -> Vec<u8> {
    encode_with_key(&keys::meta_des_key(), &to_plain(meta))
}

/// 从明文（XML 包 JSON）里提取 JSON 并解析。
fn parse_plain(text: &str) -> Result<VersionMeta, MetaError> {
    let start = text.find('{').ok_or_else(|| MetaError::BadContent("找不到 JSON 起始 `{`".into()))?;
    let end = text.rfind('}').ok_or_else(|| MetaError::BadContent("找不到 JSON 结束 `}`".into()))?;
    if end < start {
        return Err(MetaError::BadContent("JSON 括号不匹配".into()));
    }
    serde_json::from_str(&text[start..=end]).map_err(|e| MetaError::BadContent(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_KEY_HEX: &str = "0123456789abcdeffedcba9876543210";

    fn test_key() -> [u8; 16] {
        let mut k = [0u8; 16];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(&TEST_KEY_HEX[i * 2..i * 2 + 2], 16).unwrap();
        }
        k
    }

    /// 磁盘布局：magic + 密文(8 的倍数) + 小端长度；且能原样往返。
    #[test]
    fn layout_and_roundtrip() {
        let key = test_key();
        let meta = VersionMeta::new("100001900", "8847", "0.0.0.29", "2026.09.15.0000.0000_7.56");
        let plain = to_plain(&meta);
        let disk = encode_with_key(&key, &plain);

        assert_eq!(&disk[..4], &MAGIC);
        assert_eq!((disk.len() - 8) % 8, 0);
        let len = u32::from_le_bytes(disk[disk.len() - 4..].try_into().unwrap()) as usize;
        assert_eq!(len, plain.len());

        let got = decode_with_key(&key, &disk).unwrap();
        assert_eq!(got, plain);
        assert_eq!(parse_plain(std::str::from_utf8(&got).unwrap()).unwrap(), meta);
    }

    #[test]
    fn rejects_bad_magic_and_alignment() {
        let key = test_key();
        let meta = VersionMeta::new("100001900", "8847", "0.0.0.29", "view");
        let mut disk = encode_with_key(&key, &to_plain(&meta));
        disk[0] = 0;
        assert_eq!(decode_with_key(&key, &disk), Err(MetaError::BadMagic));

        let mut disk = encode_with_key(&key, &to_plain(&meta));
        disk.pop(); // 破坏 8 对齐 / 长度字段
        assert!(matches!(
            decode_with_key(&key, &disk),
            Err(MetaError::NotAligned) | Err(MetaError::BadLength)
        ));
    }
}
