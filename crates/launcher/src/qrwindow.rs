//! 二维码窗口显示（失败时由调用方回退终端渲染）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 显示中的二维码窗口，drop 时关闭。
pub struct NativeWindow {
    stop: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl Drop for NativeWindow {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // 最多等 1.5s。
        for _ in 0..30 {
            if self.finished.load(Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if let Some(h) = self.handle.take() {
            if self.finished.load(Ordering::Relaxed) {
                let _ = h.join();
            }
        }
    }
}

/// 显示 `png`，失败返回原因。
pub fn show(png: &[u8]) -> Result<NativeWindow, String> {
    if std::env::var_os("SDO_FFXIV_NO_WINDOW").is_some() {
        return Err("已按设置跳过图形窗口，改用终端二维码".to_string());
    }
    let img = crate::qrimage::decode(png)?;
    let bgra = crate::qrwin32::Bgra::from_rgb(&img.rgb, img.width, img.height);
    let stop = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let handle = crate::qrwin32::start(bgra, stop.clone(), finished.clone())?;
    Ok(NativeWindow {
        stop,
        finished,
        handle: Some(handle),
    })
}

/// 弹窗失败时的诊断信息。
pub fn environment_facts() -> String {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileType, FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE,
    };
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE};
    use windows_sys::Win32::System::StationsAndDesktops::{
        GetProcessWindowStation, GetThreadDesktop, GetUserObjectInformationW, UOI_NAME,
    };
    use windows_sys::Win32::System::Threading::GetCurrentThreadId;

    let mut facts = Vec::new();

    /// window station / desktop 的对象名（取不到给 `?`，不编造）。
    fn object_name(obj: HANDLE) -> String {
        let mut buf = [0u16; 128];
        let mut needed = 0u32;
        let ok = unsafe {
            GetUserObjectInformationW(
                obj,
                UOI_NAME,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                (buf.len() * size_of::<u16>()) as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            return "?".to_string();
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    let stdout_kind = unsafe {
        match GetFileType(GetStdHandle(STD_OUTPUT_HANDLE)) {
            FILE_TYPE_CHAR => "字符设备（真控制台）",
            FILE_TYPE_PIPE => "管道（输出被重定向）",
            FILE_TYPE_DISK => "文件",
            _ => "未知",
        }
    };
    facts.push(format!("stdout={stdout_kind}"));
    let ws = unsafe { GetProcessWindowStation() };
    facts.push(format!("window station={}", object_name(ws)));
    let desk = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
    facts.push(format!("desktop={}", object_name(desk)));
    facts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 诊断字段均非空。
    #[test]
    fn environment_facts_are_populated() {
        let f = environment_facts();
        for key in ["stdout=", "window station=", "desktop="] {
            assert!(f.contains(key), "缺少 {key} 字段：{f}");
        }
        assert!(
            !f.contains("stdout=未知"),
            "stdout 形态必须是已知类别之一：{f}"
        );
    }
}
