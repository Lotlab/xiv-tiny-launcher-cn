//! 清单 / 补丁元数据里给的相对路径：规范化 + 越界校验。
//!
//! 这些字符串来自网络（`client_all_files_list.dat`、补丁 zip 条目名、
//! `patch_delta_direct.dat`），会直接参与 `Path::join` 落盘，所以统一在这里过一遍：
//! 拒绝 `..`、盘符 / NTFS ADS（含 `:`）、以及会让 `join` 变成绝对路径的前导分隔符。

/// 规范化「相对路径」：统一 `\` → `/`，丢掉空段与 `.`，返回 `a/b/c`。
///
/// 允许前导 `/`（CDN 里表示从根算起），会被丢掉。含 `..` 或 `:` 的段、
/// 以及空路径一律返回 `None`。
pub fn safe_rel_path(raw: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for part in raw.trim().split(['/', '\\']) {
        match part {
            "" | "." => continue,
            ".." => return None,
            // `:` 出现在盘符（`C:\x`）或 NTFS 数据流（`a.txt:ads`）里。
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join("/"))
    }
}

/// 能否直接 `root.join(path)` 落盘。
///
/// 比 [`safe_rel_path`] 更严：**拒绝绝对路径**（前导 `/` / `\`），因为
/// `Path::join` 遇到绝对路径会整段替换掉 `root`。
pub fn is_safe_local_rel(raw: &str) -> bool {
    let t = raw.trim();
    if t.is_empty() || t.starts_with('/') || t.starts_with('\\') {
        return false;
    }
    safe_rel_path(t).is_some()
}

/// 比较用的规范形式：`/` 分隔 + 小写。
///
/// 清单里是 `game\sqpack\a.dat`，补丁元数据里是 `sqpack/a.dat`（相对 `game/`），
/// 两边对不上，所以比较前先过这里。
pub fn canon_path(raw: &str) -> String {
    raw.replace('\\', "/").to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_plain_relative_paths() {
        assert_eq!(safe_rel_path("game\\a.dat").as_deref(), Some("game/a.dat"));
        assert_eq!(safe_rel_path("a/b/c").as_deref(), Some("a/b/c"));
        assert_eq!(safe_rel_path("/a//b/./c").as_deref(), Some("a/b/c"));
        assert_eq!(safe_rel_path("  a/b  ").as_deref(), Some("a/b"));
    }

    #[test]
    fn rejects_traversal_drive_and_empty() {
        for bad in ["../x", "a/../../b", "..", "C:\\x", "a.txt:ads", "", "  ", "/", "./."] {
            assert_eq!(safe_rel_path(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn local_rel_rejects_absolute_paths() {
        assert!(is_safe_local_rel("game\\a.dat"));
        assert!(!is_safe_local_rel("/etc/passwd"));
        assert!(!is_safe_local_rel("\\windows\\system32\\x"));
        assert!(!is_safe_local_rel("../x"));
        assert!(!is_safe_local_rel(""));
    }

    #[test]
    fn canon_is_case_and_separator_insensitive() {
        assert_eq!(canon_path("game\\SQPACK\\A.dat"), "game/sqpack/a.dat");
        assert_eq!(canon_path("game/sqpack/a.dat"), canon_path("game\\SqPack\\A.DAT"));
    }
}
