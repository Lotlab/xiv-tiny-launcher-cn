//! `sdologinentry64.dll` — 游戏侧 64 位替代 DLL，只读本地交接返回票据与命令行。

#![allow(non_snake_case)]

mod state;
mod vtable;
mod win;

use std::ffi::c_void;
use std::sync::Once;

use proto::consts::{DLL_BUILD_MARKER, LOG_DLL};
use proto::log;

use win::Guid;

/// 构建标记，供启动器确认目标位置放的是本项目产物。
#[used]
static SDOL_BUILD_MARKER: &[u8] = DLL_BUILD_MARKER.as_bytes();

static INIT_LOG: Once = Once::new();

fn ensure_log() {
    INIT_LOG.call_once(|| {
        let _ = log::init(LOG_DLL, false, None);
    });
}

/// 校验应用信息并初始化。
/// # Safety
///
/// `p_app_info` 由游戏传入，必须指向至少 `0x20` 字节的结构。
#[no_mangle]
pub unsafe extern "system" fn SDOLInitialize(p_app_info: *mut c_void) -> i32 {
    ensure_log();
    if !win::readable(p_app_info as *const u8, 0x20) {
        return -1;
    }
    let b = p_app_info as *const u8;
    let flag = unsafe { *(b as *const u32) };
    if flag != 0x20 {
        return -1;
    }
    0
}

/// 按 IID 返回 Login / Info 对象。
#[no_mangle]
pub unsafe extern "system" fn SDOLGetModule(riid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    ensure_log();
    if ppv.is_null() {
        return -1;
    }
    unsafe {
        *ppv = std::ptr::null_mut();
    }
    let iid = win::guid_to_string(riid);
    match vtable::iid_kind(&iid) {
        Some(vtable::Iid::Login) => {
            unsafe {
                *ppv = vtable::login_obj();
            }
            0
        }
        Some(vtable::Iid::Info) => {
            unsafe {
                *ppv = vtable::info_obj();
            }
            0
        }
        None => {
            -1
        }
    }
}

/// 释放本地资源。
#[no_mangle]
pub extern "system" fn SDOLTerminal() -> i32 {
    ensure_log();
    state::clear_delivery_env();
    log::flush();
    0
}
