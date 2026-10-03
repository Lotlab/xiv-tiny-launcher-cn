//! 游戏进程启动。

use std::path::Path;

use proto::log;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, WaitForSingleObject, INFINITE, PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::winstr::wide;

pub struct Child {
    handle: HANDLE,
}

/// 启动游戏。
pub fn launch(exe: &Path, game_dir: &Path, base: &str) -> Result<Child, String> {
    if !exe.is_file() {
        return Err(format!("游戏可执行文件不存在：{}", exe.display()));
    }
    if !game_dir.is_dir() {
        return Err(format!("游戏工作目录不存在：{}", game_dir.display()));
    }
    let exe_s = exe.to_string_lossy().to_string();
    let cmdline = if exe_s.contains(' ') {
        format!("\"{exe_s}\" {base}")
    } else {
        format!("{exe_s} {base}")
    };
    let mut app = wide(&exe_s);
    let mut cmd = wide(&cmdline);
    let cwd = wide(&game_dir.to_string_lossy());

    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(
            app.as_mut_ptr(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            0,
            std::ptr::null(),
            cwd.as_ptr(),
            &si,
            &mut pi,
        );
        if ok == 0 {
            let err = GetLastError();
            log::debug(&format!("启动游戏失败细节：错误码 {err}"));
            return Err(if err == 740 {
                "启动游戏失败：需要以管理员身份运行启动器".to_string()
            } else {
                format!(
                    "启动游戏失败（错误码 {err}），请检查游戏路径与权限：{}",
                    exe.display()
                )
            });
        }
        let _ = CloseHandle(pi.hThread);
        if pi.hProcess == INVALID_HANDLE_VALUE || pi.hProcess.is_null() {
            return Err("启动游戏失败：未拿到有效的进程句柄".into());
        }
        Ok(Child {
            handle: pi.hProcess,
        })
    }
}

impl Child {
    pub fn wait(&self) {
        unsafe {
            WaitForSingleObject(self.handle, INFINITE);
        }
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}
