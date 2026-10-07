//! 瞬时替换的跨平台握手：DLL 把启动器给的 nonce 写进游戏目录里的
//! marker 文件，启动器轮询该文件内容（`LoadLibrary` 已返回 = 可安全换回官方）。
//!
//! 用文件而不用进程模块表的原因：Linux 下游戏跑在 wine/umu 里，启动器拿到的只是兼容层
//! 进程的 pid，游戏是更深层的子进程，模块表不可达；但游戏目录是同一份挂载，文件可见。
//! Windows 上模块表轮询仍保留，作为第二信号源（两者任一命中即算加载成功）。

use std::path::{Path, PathBuf};

use crate::consts::SWAP_MARKER_NAME;

/// marker 文件路径（`dir` 为游戏 `sdo\sdologin` 目录）。
pub fn marker_path(dir: &Path) -> PathBuf {
    dir.join(SWAP_MARKER_NAME)
}

/// 写入 nonce（DLL 侧在 `SDOLInitialize` 成功后调用；失败不抛，由调用方忽略）。
pub fn write_nonce(dir: &Path, nonce: &str) -> std::io::Result<()> {
    std::fs::write(marker_path(dir), nonce.as_bytes())
}

/// 读回 marker 内容（启动器侧轮询；文件不存在/不可读返回 `None`）。
pub fn read_nonce(dir: &Path) -> Option<String> {
    let bytes = std::fs::read(marker_path(dir)).ok()?;
    String::from_utf8(bytes).ok().filter(|s| !s.is_empty())
}

/// 内容是否为本次运行的 nonce（陈旧 marker 永不命中）。
pub fn nonce_matches(dir: &Path, nonce: &str) -> bool {
    read_nonce(dir).as_deref() == Some(nonce)
}

/// 删除 marker（尽力而为）。
pub fn remove(dir: &Path) {
    let _ = std::fs::remove_file(marker_path(dir));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xivtl-marker-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn nonce_roundtrip() {
        let dir = tmp();
        assert!(!nonce_matches(&dir, "abc"));
        write_nonce(&dir, "abc").unwrap();
        assert!(nonce_matches(&dir, "abc"));
        assert!(!nonce_matches(&dir, "abd"), "旧 nonce 不能命中");
        remove(&dir);
        assert!(!nonce_matches(&dir, "abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_marker_never_matches() {
        let dir = tmp();
        std::fs::write(marker_path(&dir), [0xFF, 0xFE, 0x00]).unwrap();
        assert!(!nonce_matches(&dir, "abc"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
