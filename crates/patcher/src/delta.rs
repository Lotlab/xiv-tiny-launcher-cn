//! delta 应用：用 `rxdelta`（纯 Rust VCDIFF/xdelta3 兼容解码器）在**原地**打补丁。
//!
//! 选 `apply_paths_in_place_verified` 的原因：
//! - 只重写变化的窗口，10GB 级 pack 不需要再复制一份；
//! - 自带 journal，断电可恢复；
//! - `expect_before`/`expect_after` 分别对应元数据的 `origin_md5` / `result_md5`，
//!   并且源文件已经是目标内容时会**幂等跳过**。
//!
//! 调用前先按元数据的 `delta_md5` 校验 delta 自身（[`apply_in_place`] 里做），
//! 不符就不动源文件、交给链尾整文件补下。

use std::path::Path;

use rxdelta::{ApplyOptions, ChecksumAlgo, InPlaceOutcome};

use crate::disk;
use crate::hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaError {
    /// 元数据里的 MD5 不是 32 位 hex。
    BadMd5(String),
    /// delta 文件与清单里的 `delta_md5` 不符（缺失 / 被改 / 下坏）。
    BadDelta(String),
    /// 应用失败（源 MD5 不符、delta 损坏等）。
    Apply(String),
    /// 磁盘空间不够。
    NoSpace { need: u64, free: u64 },
}

impl std::fmt::Display for DeltaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeltaError::BadMd5(s) => write!(f, "MD5 非法：{s}"),
            DeltaError::BadDelta(s) => write!(f, "delta 与清单不符：{s}"),
            DeltaError::Apply(e) => write!(f, "delta 应用失败：{e}"),
            DeltaError::NoSpace { need, free } => write!(
                f,
                "磁盘空间不足：需要 {}，可用 {}",
                disk::human(*need),
                disk::human(*free)
            ),
        }
    }
}

impl std::error::Error for DeltaError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaOutcome {
    /// 本次打了补丁。
    Applied,
    /// 源文件已经是目标内容，跳过。
    Skipped,
}

/// 把 `delta_file` 原地打到 `game_file` 上。
///
/// 三道校验：delta 自身（`delta_md5`，空则跳过）、打之前（`origin_md5`）、
/// 打之后（`result_md5`）。源文件已是目标内容时 rxdelta 会幂等跳过。
pub fn apply_in_place(
    game_file: &Path,
    delta_file: &Path,
    delta_md5_hex: &str,
    origin_md5_hex: &str,
    result_md5_hex: &str,
) -> Result<DeltaOutcome, DeltaError> {
    // 先确认 delta 就是清单里那一个（内容/长度都要对），再动源文件。
    if !crate::cdn::verify_file(delta_file, 0, delta_md5_hex) {
        return Err(DeltaError::BadDelta(format!(
            "{}（期望 MD5 {delta_md5_hex}）",
            delta_file.display()
        )));
    }

    if let Some(need) = required_bytes(game_file, delta_file) {
        disk::check_free_space(game_file, need)
            .map_err(|(need, free)| DeltaError::NoSpace { need, free })?;
    }

    let before = md5_bytes(origin_md5_hex)?;
    let after = md5_bytes(result_md5_hex)?;
    let opts = ApplyOptions::default();
    match rxdelta::apply_paths_in_place_verified(
        game_file,
        delta_file,
        &opts,
        ChecksumAlgo::Md5,
        Some(&before),
        Some(&after),
    ) {
        Ok(InPlaceOutcome::Applied { .. }) => Ok(DeltaOutcome::Applied),
        Ok(InPlaceOutcome::Skipped { .. }) => Ok(DeltaOutcome::Skipped),
        Err(e) => Err(DeltaError::Apply(e.to_string())),
    }
}

/// 打这个 delta 需要的额外磁盘空间（含余量）。
///
/// - 原地重写：要落一份 journal（被改写区间的原始字节，约等于变更量），加上文件增长量；
/// - 变更量超过 rxdelta 的原地上限时它会回退成 temp+rename，那就需要整个目标大小。
fn required_bytes(game_file: &Path, delta_file: &Path) -> Option<u64> {
    let src_len = std::fs::metadata(game_file).ok()?.len();
    let map = rxdelta::io::MappedFile::open(delta_file).ok()?;
    let layout = rxdelta::scan_layout(map.as_bytes(), rxdelta::DEFAULT_MAX_WINDOW).ok()?;
    let need = if layout.changed_bytes <= rxdelta::DEFAULT_IN_PLACE_THRESHOLD {
        layout.changed_bytes + layout.target_len.saturating_sub(src_len)
    } else {
        layout.target_len
    };
    Some(need + disk::SPACE_MARGIN)
}

fn md5_bytes(hex: &str) -> Result<[u8; 16], DeltaError> {
    hash::decode_hex_array::<16>(hex)
        .ok_or_else(|| DeltaError::BadMd5(format!("应为 32 位 hex：{hex}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_hex_parsing() {
        let a = md5_bytes("00FF10AB000000000000000000000000").unwrap();
        assert_eq!(a[0], 0x00);
        assert_eq!(a[1], 0xFF);
        assert_eq!(a[2], 0x10);
        assert_eq!(a[3], 0xAB);
        assert!(matches!(md5_bytes("xyz"), Err(DeltaError::BadMd5(_))));
        assert!(matches!(md5_bytes("00"), Err(DeltaError::BadMd5(_))));
    }

    /// delta 与清单不符（含文件不存在）时不动源文件，直接报错。
    #[test]
    fn rejects_delta_that_does_not_match_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let game_file = dir.path().join("a.dat");
        let delta_file = dir.path().join("a.dat.delta");
        std::fs::write(&game_file, b"source").unwrap();
        std::fs::write(&delta_file, b"not the delta").unwrap();

        // 清单里的 delta_md5 与实际文件不符。
        let err = apply_in_place(&game_file, &delta_file, &"00".repeat(16), "", "");
        assert!(matches!(err, Err(DeltaError::BadDelta(_))), "{err:?}");
        assert_eq!(std::fs::read(&game_file).unwrap(), b"source", "源文件不该被改");

        // delta 文件根本不存在。
        let err = apply_in_place(&game_file, &dir.path().join("missing.delta"), "", "", "");
        assert!(matches!(err, Err(DeltaError::BadDelta(_))), "{err:?}");
    }
}
