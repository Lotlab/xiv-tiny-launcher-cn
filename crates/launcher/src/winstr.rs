//! Win32 宽字符串。

/// 以 NUL 结尾的 UTF-16 序列。
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
