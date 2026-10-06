//! 进程检查：更新前确认游戏没在运行，否则文件被占用（尤其 Windows 上
//! `ffxiv_dx11.exe` 与 sqpack 会被独占打开）。

/// 是否存在名为 `exe_name`（不区分大小写）的进程。
#[cfg(windows)]
pub fn is_running(exe_name: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let want = exe_name.to_ascii_lowercase();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            // 查不到就当没在跑：不该因为查询失败而拦住更新。
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let raw = &entry.szExeFile;
            let len = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
            let name = String::from_utf16_lossy(&raw[..len]);
            if name.to_ascii_lowercase() == want {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// 非 Windows：扫 `/proc/<pid>/comm`。
#[cfg(not(windows))]
pub fn is_running(exe_name: &str) -> bool {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for e in entries.flatten() {
        let pid = e.file_name();
        let pid = pid.to_string_lossy();
        if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
            if comm.trim().eq_ignore_ascii_case(exe_name) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonexistent_process_is_not_running() {
        assert!(!is_running("this-process-name-should-not-exist-9f3a"));
    }

    #[cfg(not(windows))]
    #[test]
    fn finds_self_by_comm() {
        // `/proc/<pid>/comm` 被内核截断到 15 字符；真实目标 `ffxiv_dx11.exe` 只有 14 字符。
        let me = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default();
        if me.is_empty() {
            return;
        }
        let comm: String = me.chars().take(15).collect();
        assert!(is_running(&comm), "应能找到自己：{comm}");
    }
}
