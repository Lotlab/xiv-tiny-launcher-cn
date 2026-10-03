//! 单实例：后启动的实例直接退出。互斥体名见 `proto::consts::MUTEX_NAME`。

use proto::consts::MUTEX_NAME;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

use crate::winstr::wide;

pub struct InstanceGuard {
    handle: HANDLE,
}

pub enum Acquire {
    /// 拿到互斥体，本次是本机唯一的实例。
    Guard(InstanceGuard),
    AlreadyRunning,
    /// 创建互斥体失败（OS 错误码）。
    Failed(u32),
}

pub fn acquire() -> Acquire {
    let name = wide(MUTEX_NAME);
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Acquire::Failed(unsafe { GetLastError() });
    }
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Acquire::AlreadyRunning;
    }
    Acquire::Guard(InstanceGuard { handle })
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}
