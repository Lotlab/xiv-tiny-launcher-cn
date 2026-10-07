//! swap 握手 marker：`SDO_FFXIV_SWAP` 环境变量存在时，把它的值
//! 写进本 DLL 所在目录的 marker 文件，证明 `LoadLibrary` 已返回、启动器可以换回官方。
//!
//! 文件读写纯尽力而为：任何失败都静默忽略，绝不影响登录主流程。

use proto::consts::ENV_SWAP;
use proto::swap_marker;

use crate::win;

/// `SDOLInitialize` 成功后调用：有握手请求才写 marker。
pub fn notify_if_requested() {
    let Some(nonce) = win::get_env(ENV_SWAP) else {
        return;
    };
    let Some(dir) = own_module_dir() else {
        return;
    };
    let _ = swap_marker::write_nonce(&dir, &nonce);
}

/// `SDOLTerminal` 时调用：顺手清掉 marker（清不掉由启动器兜底）。
pub fn clear_if_requested() {
    if win::get_env(ENV_SWAP).is_none() {
        return;
    }
    if let Some(dir) = own_module_dir() {
        swap_marker::remove(&dir);
    }
}

/// 本 DLL 所在目录（`GetModuleFileNameW` 截掉文件名）。
fn own_module_dir() -> Option<std::path::PathBuf> {
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::{
        GetModuleFileNameW, GetModuleHandleExW, GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
    };

    unsafe {
        let mut h: HMODULE = std::ptr::null_mut();
        // 以本模块内静态量的地址反查模块句柄（与 DLL 文件名解耦）。
        static PROBE: u8 = 0;
        let addr = std::ptr::addr_of!(PROBE) as *const u16;
        if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, addr, &mut h) == 0
        {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        let n = GetModuleFileNameW(h, buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 || n as usize >= buf.len() {
            return None;
        }
        buf.truncate(n as usize);
        let path = String::from_utf16(&buf).ok()?;
        std::path::Path::new(&path)
            .parent()
            .map(|p| p.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 纯路径逻辑不依赖 loader：写/读/删一遍（写进系统临时目录，不污染游戏目录）。
    #[test]
    fn marker_file_roundtrip() {
        // 非 Windows 下这个 crate 编译为空，此测试只在 Windows 跑。
        let dir = std::env::temp_dir().join(format!("xivtl-dllmarker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        swap_marker::write_nonce(&dir, "NONCE123").unwrap();
        assert!(swap_marker::nonce_matches(&dir, "NONCE123"));
        swap_marker::remove(&dir);
        assert!(!swap_marker::nonce_matches(&dir, "NONCE123"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无握手 env 时不写任何文件（按 DLL 所在目录推算 must not 产生 marker）。
    #[test]
    fn no_env_no_marker() {
        let _g = crate::state::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(ENV_SWAP);
        // 即使调了也必须是空操作（无 env 直接返回，不碰文件系统）。
        notify_if_requested();
        clear_if_requested();
    }
}
