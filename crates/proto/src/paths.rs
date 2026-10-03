//! 本地路径与原子写。

use std::path::{Path, PathBuf};

/// EXE 当前目录下的文件（`device.json` / `server.json` / `qrcode.png`）。
pub fn cwd_file(name: &str) -> PathBuf {
    let mut p = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    p.push(name);
    p
}

/// 日志目录 `%TEMP%/SdoFfxiv/`。
pub(crate) fn log_dir() -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(crate::consts::LOG_DIR_NAME);
    p
}

/// 日志文件首选路径 `%TEMP%/SdoFfxiv/{name}`。
pub(crate) fn log_file(name: &str) -> PathBuf {
    log_dir().join(name)
}

/// 日志文件候选：先 `%TEMP%/SdoFfxiv/{name}`，不可写时退到 EXE 所在目录。
pub fn log_file_candidates(name: &str) -> Vec<PathBuf> {
    let mut v = vec![log_file(name)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let alt = dir.join(name);
            if !v.contains(&alt) {
                v.push(alt);
            }
        }
    }
    v
}

/// 原子写：写 `*.tmp` 后改名（Windows 上 `rename` 可覆盖已存在目标）。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = {
        let mut s = path.as_os_str().to_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    };
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_roundtrip() {
        let mut p = std::env::temp_dir();
        p.push(format!("xivtl-test-{}.json", std::process::id()));
        write_atomic(&p, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"{\"a\":1}");
        // 覆盖写（rename 替换）
        write_atomic(&p, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"{\"a\":2}");
        let _ = std::fs::remove_file(&p);
    }
}
