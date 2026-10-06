//! 磁盘剩余空间查询（打 delta / 下 zip 前预检，避免写到一半空间不够）。
//!
//! 查不到时返回 `None`，调用方按「不拦」处理——不该因为查询失败而阻止更新。

use std::path::Path;

/// 空间预检留的安全余量。
pub const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

/// 检查 `path` 所在卷是否有 `need` 字节可用。
///
/// 查不到可用空间时不拦（同 [`free_space`]）；不够则返回 `(need, free)`。
pub fn check_free_space(path: &Path, need: u64) -> Result<(), (u64, u64)> {
    match free_space(path) {
        Some(free) if free < need => Err((need, free)),
        _ => Ok(()),
    }
}

/// `path` 所在卷的可用字节数（给非特权用户的可用量）。
pub fn free_space(path: &Path) -> Option<u64> {
    // 传目录（或文件的父目录）给系统调用。
    let target = if path.is_dir() {
        path
    } else {
        path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(path)
    };
    free_space_impl(target)
}

#[cfg(windows)]
fn free_space_impl(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free as *mut u64 as *mut _,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        None
    } else {
        Some(free)
    }
}

#[cfg(unix)]
fn free_space_impl(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return None;
    }
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

#[cfg(not(any(windows, unix)))]
fn free_space_impl(_dir: &Path) -> Option<u64> {
    None
}

/// 人类可读的字节数。
pub fn human(bytes: u64) -> String {
    const GB: f64 = 1e9;
    const MB: f64 = 1e6;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_space_is_sane() {
        let p = std::env::temp_dir();
        match free_space(&p) {
            Some(n) => assert!(n > 0, "可用空间应大于 0：{n}"),
            None => eprintln!("跳过：查不到 {} 的可用空间", p.display()),
        }
    }

    #[test]
    fn human_formats() {
        assert_eq!(human(500), "500 B");
        assert_eq!(human(1_500_000), "1.5 MB");
        assert_eq!(human(2_500_000_000), "2.50 GB");
    }
}
