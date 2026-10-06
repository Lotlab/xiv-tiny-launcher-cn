//! 版本比较与本地版本读取。

use std::cmp::Ordering;
use std::path::Path;

use crate::version_meta;

/// 点分段数值比较（短的一方缺位补 0）。
///
/// 与参考实现一致：`2026.09.15.0000.0000 > 2026.09.01.0000.0000`。
/// 解析不出来的段按 0 算（**不能丢掉**：丢段会让后面的位错位）。
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let pa: Vec<u32> = a.split('.').map(|s| s.trim().parse().unwrap_or(0)).collect();
    let pb: Vec<u32> = b.split('.').map(|s| s.trim().parse().unwrap_or(0)).collect();
    for i in 0..pa.len().max(pb.len()) {
        let va = pa.get(i).copied().unwrap_or(0);
        let vb = pb.get(i).copied().unwrap_or(0);
        match va.cmp(&vb) {
            Ordering::Equal => continue,
            other => return other,
        }
    }
    Ordering::Equal
}

/// 版本检查结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    /// 已是最新。
    UpToDate,
    /// 有更新（本地缺失也算）。
    UpdateAvailable,
    /// 本地比 CDN 还新（不降级）。
    LocalNewer,
}

pub fn decide(remote: &str, local: Option<&str>) -> UpdateDecision {
    match local {
        None => UpdateDecision::UpdateAvailable,
        Some(l) => match compare_versions(remote, l) {
            Ordering::Greater => UpdateDecision::UpdateAvailable,
            Ordering::Equal => UpdateDecision::UpToDate,
            Ordering::Less => UpdateDecision::LocalNewer,
        },
    }
}

/// 本地版本从哪读到的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalVersionSource {
    /// `game/LocalVersion3.xml`（加密元数据）。
    Meta,
    /// `game/ffxivgame.ver`（纯文本 display 版本）。
    VerFile,
}

/// 本地版本。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalVersion {
    /// display 版本（已去掉 `_name` 后缀）。
    pub display: String,
    /// internal 版本（只有元数据里有）。
    pub internal: Option<String>,
    pub source: LocalVersionSource,
}

/// `2026.09.15.0000.0000_7.56` → `2026.09.15.0000.0000`。
fn display_of(view: &str) -> String {
    view.split('_').next().unwrap_or(view).trim().to_string()
}

/// 读本地版本：优先 `game/LocalVersion3.xml`，回退 `game/ffxivgame.ver`。
///
/// 返回 `Ok(None)` 表示两者都不存在（全新安装）。
pub fn read_local(root: &Path) -> Result<Option<LocalVersion>, String> {
    let game = root.join("game");

    let meta_path = game.join("LocalVersion3.xml");
    if meta_path.is_file() {
        match std::fs::read(&meta_path) {
            Ok(bytes) => match version_meta::parse(&bytes) {
                Ok(meta) => {
                    return Ok(Some(LocalVersion {
                        display: display_of(&meta.version.view),
                        internal: Some(meta.version.v),
                        source: LocalVersionSource::Meta,
                    }))
                }
                Err(e) => {
                    // 元数据坏了不该让整个检查失败：记下来，回退 ver 文件。
                    proto::log::warn(&format!(
                        "{} 解析失败（{e}），回退 ffxivgame.ver",
                        meta_path.display()
                    ));
                }
            },
            Err(e) => proto::log::warn(&format!("{} 读取失败：{e}", meta_path.display())),
        }
    }

    let ver_path = game.join("ffxivgame.ver");
    if ver_path.is_file() {
        let text = std::fs::read_to_string(&ver_path)
            .map_err(|e| format!("{} 读取失败：{e}", ver_path.display()))?;
        let display = display_of(text.trim());
        if !display.is_empty() {
            return Ok(Some(LocalVersion {
                display,
                internal: None,
                source: LocalVersionSource::VerFile,
            }));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_pads_missing_segments() {
        assert_eq!(compare_versions("7.2", "7.20"), Ordering::Less);
        assert_eq!(compare_versions("7.20", "7.2"), Ordering::Greater);
        assert_eq!(compare_versions("2026.09.15.0000.0000", "2026.09.01.0000.0000"), Ordering::Greater);
        assert_eq!(compare_versions("7.30", "7.30.1"), Ordering::Less);
        assert_eq!(compare_versions("7.30.1", "7.30"), Ordering::Greater);
        assert_eq!(compare_versions("2026.09.15.0000.0000", "2026.09.15.0000.0000"), Ordering::Equal);
        // 非数字段按 0 算，但**不能**把该段丢掉：
        // 丢掉 "2a" 会让 "1.2a.4" 变成 [1,4] > [1,2,3]，错判成 Greater。
        assert_eq!(compare_versions("1.2a.3", "1.2.3"), Ordering::Less); // 2a → 0
        assert_eq!(compare_versions("1.2a.4", "1.2.3"), Ordering::Less);
    }

    #[test]
    fn decide_covers_all_cases() {
        assert_eq!(decide("2026.09.15.0000.0000", None), UpdateDecision::UpdateAvailable);
        assert_eq!(decide("2026.09.15.0000.0000", Some("2026.09.01.0000.0000")), UpdateDecision::UpdateAvailable);
        assert_eq!(decide("2026.09.15.0000.0000", Some("2026.09.15.0000.0000")), UpdateDecision::UpToDate);
        assert_eq!(decide("2026.09.01.0000.0000", Some("2026.09.15.0000.0000")), UpdateDecision::LocalNewer);
    }

    #[test]
    fn display_strips_name_suffix() {
        assert_eq!(display_of("2026.09.15.0000.0000_7.56"), "2026.09.15.0000.0000");
        assert_eq!(display_of("2026.09.15.0000.0000"), "2026.09.15.0000.0000");
        assert_eq!(display_of(" 1.2.3 \n"), "1.2.3");
    }

    #[test]
    fn reads_ver_file_when_no_meta() {
        let dir = tempfile::tempdir().unwrap();
        let game = dir.path().join("game");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(game.join("ffxivgame.ver"), "2026.09.15.0000.0000\n").unwrap();
        let v = read_local(dir.path()).unwrap().unwrap();
        assert_eq!(v.display, "2026.09.15.0000.0000");
        assert_eq!(v.source, LocalVersionSource::VerFile);
        assert_eq!(v.internal, None);
    }

    #[test]
    fn missing_everything_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_local(dir.path()).unwrap(), None);
    }
}
