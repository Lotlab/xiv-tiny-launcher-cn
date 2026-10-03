//! SSO Cookie 写入/清理：走系统 Cookie 存储，游戏进程读的就是这一份。

use proto::consts::*;
use proto::log;
use windows_sys::Win32::Networking::WinInet::InternetSetCookieW;

use crate::winstr::wide;

fn set(url: &str, data: &str) -> bool {
    let u = wide(url);
    let d = wide(data);
    unsafe { InternetSetCookieW(u.as_ptr(), std::ptr::null(), d.as_ptr()) != 0 }
}

/// 换票成功后写入系统 Cookie。
pub fn write_sso_cookies(ticket: &str) {
    let cas = format!("{COOKIE_CAS_NAME}={ticket};path=/;Domain={COOKIE_DOMAIN_CAS};");
    let state = format!("{COOKIE_STATE_NAME}=1;path=/;Domain={COOKIE_DOMAIN_SDO};");
    let ok1 = set(COOKIE_URL_CAS, &cas);
    let ok2 = set(COOKIE_URL_SDO, &state);
    if !ok1 || !ok2 {
        log::warn("登录状态同步到系统失败，仍会继续启动");
    }
}

/// 退出时清理系统 Cookie。
pub fn clear_sso_cookies() {
    let cas =
        format!("{COOKIE_CAS_NAME}=;path=/;Domain={COOKIE_DOMAIN_CAS};expires={COOKIE_EXPIRED}");
    let state =
        format!("{COOKIE_STATE_NAME}=;path=/;Domain={COOKIE_DOMAIN_SDO};expires={COOKIE_EXPIRED}");
    let ok1 = set(COOKIE_URL_CAS, &cas);
    let ok2 = set(COOKIE_URL_SDO, &state);
    if !ok1 || !ok2 {
        log::warn("退出清理未完全成功，下次启动会自动覆盖");
    }
}
