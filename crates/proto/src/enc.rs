//! 编码与摘要。
//!
//! 只有三个参数做 URL 编码：`epName`、`productVersion`（CAS 链）、`inputUserId`。
//! 其余参数一律原文发送（`deviceId` 的 `:`、`macId` 的 `-`、`epIp` 的 `.` 都不编码）。

/// UTF-8 字节 → URL 编码：`A-Za-z0-9` 直通，空格 → `+`，其余 `%XX` 大写。
///
/// 非 RFC 3986：`~`/`_`/`-`/`.` 也会被编码。
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => out.push(*b as char),
            b' ' => out.push('+'),
            other => {
                out.push('%');
                out.push(hex_digit(other >> 4));
                out.push(hex_digit(other & 0x0F));
            }
        }
    }
    out
}

fn hex_digit(v: u8) -> char {
    match v {
        0..=9 => (b'0' + v) as char,
        _ => (b'A' + (v - 10)) as char,
    }
}

/// 字节切片 → 大写十六进制。
pub(crate) fn hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(hex_digit(b >> 4));
        s.push(hex_digit(b & 0x0F));
    }
    s
}

/// 随机 `u64`（轮询抖动等非安全用途）。失败时退化到时钟，避免 panic。
pub fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    if getrandom::fill(&mut buf).is_err() {
        fill_from_clock(&mut buf);
    }
    u64::from_le_bytes(buf)
}

/// 随机 `n` 字节 → 大写十六进制（用于 `SEG1` / `runTimeId`）。
pub fn random_hex_upper(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        fill_from_clock(&mut buf);
    }
    hex_upper(&buf)
}

/// PRNG 不可用时用当前时间铺满 `buf`：只为保持格式合法且不 panic，不做任何安全用途。
pub(crate) fn fill_from_clock(buf: &mut [u8]) {
    let nanos = crate::clock::now_nanos();
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (nanos >> ((i % 8) * 8)) as u8;
    }
}

/// `MD5(macId)` 大写十六进制（`deviceId` 的 `SEG0`）。
pub(crate) fn md5_hex_upper(s: &str) -> String {
    use md5::{Digest, Md5};
    let mut h = Md5::new();
    h.update(s.as_bytes());
    hex_upper(&h.finalize())
}

/// 32 位大写十六进制、不带连字符的 GUID 字符串（`runTimeId`）。
pub fn new_run_time_id() -> String {
    random_hex_upper(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_rules() {
        assert_eq!(
            url_encode("user_name@example.com"),
            "user%5Fname%40example%2Ecom"
        );
        assert_eq!(url_encode("DESKTOP-ABC1234"), "DESKTOP%2DABC1234");
        assert_eq!(url_encode("a b"), "a+b");
        assert_eq!(url_encode("~"), "%7E");
        assert_eq!(url_encode("AZaz09"), "AZaz09");
        // 中文 → UTF-8 多字节逐字节编码
        assert_eq!(url_encode("中"), "%E4%B8%AD");
    }

    #[test]
    fn md5_known_vector() {
        assert_eq!(md5_hex_upper(""), "D41D8CD98F00B204E9800998ECF8427E");
        assert_eq!(
            md5_hex_upper("00-11-22-33-44-55"),
            "88440FF9DD5D5C6819D6D4652279A1BC"
        );
    }

    #[test]
    fn runtime_id_shape() {
        let id = new_run_time_id();
        assert_eq!(id.len(), 32);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_lowercase()));
    }
}
