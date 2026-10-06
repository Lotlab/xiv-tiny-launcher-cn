//! MD5 与 hex 小工具：全 crate 共用一份，避免各处自己拼十六进制。

use md5::{Digest, Md5};

/// 摘要 → 大写 hex。
pub fn hex_upper(digest: &[u8]) -> String {
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        out.push_str(&format!("{b:02X}"));
    }
    out
}

/// 一段字节的 MD5（大写 hex）。
pub fn md5_hex_upper(data: &[u8]) -> String {
    let mut h = Md5::new();
    h.update(data);
    hex_upper(&h.finalize())
}

/// 解码 hex（大小写均可，允许首尾空白）；奇数长度或非法字符返回 `None`。
pub fn decode_hex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for i in (0..s.len()).step_by(2) {
        out.push(u8::from_str_radix(&s[i..i + 2], 16).ok()?);
    }
    Some(out)
}

/// 解码成固定长度数组；长度不符或含非法字符返回 `None`。
pub fn decode_hex_array<const N: usize>(s: &str) -> Option<[u8; N]> {
    decode_hex(s)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_rejects() {
        assert_eq!(md5_hex_upper(b"hello"), "5D41402ABC4B2A76B9719D911017C592");
        assert_eq!(decode_hex("5d41402abc4b2a76b9719d911017c592").unwrap().len(), 16);
        assert_eq!(decode_hex_array::<16>("00ff10ab000000000000000000000000").unwrap()[1], 0xFF);
        assert_eq!(decode_hex("xyz"), None);
        assert_eq!(decode_hex("00"), Some(vec![0]));
        assert_eq!(decode_hex_array::<16>("00"), None);
    }
}
