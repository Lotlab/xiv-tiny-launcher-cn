//! 虚表布局与槽位实现。游戏实际只调 `Login[10]/[24]`、`Info[3]`。

use std::ffi::c_void;

use proto::consts::{ENV_AREAID, ENV_BASE, ENV_SNDAID, ENV_TICKET};

use crate::consts::{IID_INFO, IID_LOGIN};
use proto::log;

use crate::state;
use crate::win::{self, Guid};

type P = *mut c_void;
type Pw = *mut u16;

unsafe extern "system" fn q0(_t: P, _iid: *const Guid, ppv: *mut *mut c_void) -> i32 {
    if !ppv.is_null() {
        *ppv = std::ptr::null_mut();
    }
    1
}
unsafe extern "system" fn q1(_t: P) -> u32 {
    1
}
unsafe extern "system" fn q2(_t: P) -> u32 {
    1
}
unsafe extern "system" fn f3_ok(_t: P, _hwnd: isize) -> i32 {
    0
}
unsafe extern "system" fn f4_show_login_dialog(_t: P, _cb: P, _rsv: P) -> i32 {
    if state::delivery_ready() {
        0 // S_OK
    } else {
        0x8000_000Au32 as i32 // E_PENDING
    }
}
unsafe extern "system" fn f5_close(_t: P) -> i32 {
    0
}
unsafe extern "system" fn f6_move(_t: P, _x: i32, _y: i32) -> i32 {
    0
}
unsafe extern "system" fn f7_logout(_t: P) -> i32 {
    state::clear_delivery_env();
    log::info("Logout：已清理登录信息");
    0
}
unsafe extern "system" fn f8_do_login(_t: P) -> i32 {
    if state::delivery_ready() {
        0
    } else {
        -1
    }
}
unsafe extern "system" fn f9_ok(_t: P, _mode: i32) -> i32 {
    0
}
unsafe extern "system" fn f10_get_ticket(_t: P, ticket: *mut Pw, snda: *mut Pw) -> i32 {
    fill_ticket(ticket, snda)
}
unsafe extern "system" fn f11_ok(_t: P, _p: P) -> i32 {
    0
}
unsafe extern "system" fn f12_log_bstr(_t: P, _b: Pw) -> i32 {
    0
}
unsafe extern "system" fn f13_log3(_t: P, _s: *const u16, _a: i32, _b: i32) -> i32 {
    0
}
unsafe extern "system" fn f14_ok(_t: P) -> i32 {
    0
}
unsafe extern "system" fn f15_empty_bstr(_t: P) -> Pw {
    win::bstr("")
}
unsafe extern "system" fn f16_ok(_t: P, _cb: i64) -> i32 {
    0
}
unsafe extern "system" fn f17_ok(_t: P, _a: P, _b: P) -> i32 {
    0
}
unsafe extern "system" fn f18_ok(_t: P) -> i32 {
    0
}
unsafe extern "system" fn f19_ok(_t: P, _v: i32) -> i32 {
    0
}
unsafe extern "system" fn f20_set_client_type(_t: P, _b: Pw) -> i32 {
    0
}
unsafe extern "system" fn f21_ok(_t: P) -> i32 {
    0
}
unsafe extern "system" fn f22_auth_code_login(_t: P, _a: P, _b: P) -> i32 {
    -1
}
unsafe extern "system" fn f23_session_login_game(_t: P, _a: P, _b: P, _c: P, _d: P) -> i32 {
    if state::delivery_ready() {
        0
    } else {
        -1
    }
}
unsafe extern "system" fn f24_get_ticket_for_appid(
    _t: P,
    ticket: *mut Pw,
    snda: *mut Pw,
    _app_id: i32,
) -> i32 {
    fill_ticket(ticket, snda)
}
unsafe extern "system" fn f25_ok(_t: P, _a: P) -> i32 {
    0
}

/// 取票据：有就返回，没就失败，出参先置空。
fn fill_ticket(ticket: *mut Pw, snda: *mut Pw) -> i32 {
    unsafe {
        if !ticket.is_null() {
            *ticket = std::ptr::null_mut();
        }
        if !snda.is_null() {
            *snda = std::ptr::null_mut();
        }
    }
    if ticket.is_null() || snda.is_null() {
        return -102;
    }
    let t = match win::get_env(ENV_TICKET) {
        Some(v) => v,
        None => {
            return -1;
        }
    };
    let s = match win::get_env(ENV_SNDAID) {
        Some(v) => v,
        None => {
            return -1;
        }
    };
    unsafe {
        *ticket = win::bstr(&t);
        *snda = win::bstr(&s);
    }
    0
}

unsafe extern "system" fn i0(_t: P) -> i32 {
    1
}
unsafe extern "system" fn i1(_t: P) -> i32 {
    1
}
unsafe extern "system" fn i2(_t: P) -> i32 {
    1
}
unsafe extern "system" fn i3_get_command_line(_t: P, base: *mut Pw, area: *mut i32) -> i32 {
    unsafe {
        if !base.is_null() {
            *base = std::ptr::null_mut();
        }
    }
    if base.is_null() || area.is_null() {
        return -102;
    }
    let b = match win::get_env(ENV_BASE) {
        Some(v) => v,
        None => {
            return -1;
        }
    };
    let a = win::get_env(ENV_AREAID)
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);
    unsafe {
        *base = win::bstr(&b);
        *area = a;
    }
    0
}
unsafe extern "system" fn i4_ok(_t: P, _b: Pw) -> i32 {
    0
}
unsafe extern "system" fn i5_ok(_t: P, _p: *mut u32) -> i32 {
    0
}
unsafe extern "system" fn i6_ok(_t: P, _p: *mut i32) -> i32 {
    0
}
unsafe extern "system" fn i7_ok(_t: P, _p: *mut i32) -> i32 {
    0
}
unsafe extern "system" fn i8_ok(_t: P, _b: Pw) -> i32 {
    0
}
unsafe extern "system" fn i9_ok(_t: P, _b: Pw) -> i32 {
    0
}

#[repr(C)]
pub struct LoginVtbl {
    pub q0: unsafe extern "system" fn(P, *const Guid, *mut *mut c_void) -> i32, // [0x00]
    pub q1: unsafe extern "system" fn(P) -> u32,                                // [0x08]
    pub q2: unsafe extern "system" fn(P) -> u32,                                // [0x10]
    pub f3: unsafe extern "system" fn(P, isize) -> i32,                         // [0x18]
    pub f4: unsafe extern "system" fn(P, P, P) -> i32, // [0x20] S_OK/E_PENDING
    pub f5: unsafe extern "system" fn(P) -> i32,       // [0x28]
    pub f6: unsafe extern "system" fn(P, i32, i32) -> i32, // [0x30]
    pub f7: unsafe extern "system" fn(P) -> i32,       // [0x38] Logout
    pub f8: unsafe extern "system" fn(P) -> i32,       // [0x40] DoLogin
    pub f9: unsafe extern "system" fn(P, i32) -> i32,  // [0x48]
    pub f10: unsafe extern "system" fn(P, *mut Pw, *mut Pw) -> i32, // [0x50] GetTicket
    pub f11: unsafe extern "system" fn(P, P) -> i32,   // [0x58]
    pub f12: unsafe extern "system" fn(P, Pw) -> i32,  // [0x60]
    pub f13: unsafe extern "system" fn(P, *const u16, i32, i32) -> i32, // [0x68]
    pub f14: unsafe extern "system" fn(P) -> i32,      // [0x70]
    pub f15: unsafe extern "system" fn(P) -> Pw,       // [0x78] 空 BSTR
    pub f16: unsafe extern "system" fn(P, i64) -> i32, // [0x80]
    pub f17: unsafe extern "system" fn(P, P, P) -> i32, // [0x88]
    pub f18: unsafe extern "system" fn(P) -> i32,      // [0x90]
    pub f19: unsafe extern "system" fn(P, i32) -> i32, // [0x98]
    pub f20: unsafe extern "system" fn(P, Pw) -> i32,  // [0xA0]
    pub f21: unsafe extern "system" fn(P) -> i32,      // [0xA8]
    pub f22: unsafe extern "system" fn(P, P, P) -> i32, // [0xB0]
    pub f23: unsafe extern "system" fn(P, P, P, P, P) -> i32, // [0xB8]
    pub f24: unsafe extern "system" fn(P, *mut Pw, *mut Pw, i32) -> i32, // [0xC0] ForAppid
    pub f25: unsafe extern "system" fn(P, P) -> i32,   // [0xC8]
}

#[repr(C)]
pub struct InfoVtbl {
    pub i0: unsafe extern "system" fn(P) -> i32, // [0x00] Return1
    pub i1: unsafe extern "system" fn(P) -> i32, // [0x08] Return1
    pub i2: unsafe extern "system" fn(P) -> i32, // [0x10] Return1
    pub f3: unsafe extern "system" fn(P, *mut Pw, *mut i32) -> i32, // [0x18] baseCmd+areaId
    pub f4: unsafe extern "system" fn(P, Pw) -> i32, // [0x20]
    pub f5: unsafe extern "system" fn(P, *mut u32) -> i32, // [0x28] 恒0
    pub f6: unsafe extern "system" fn(P, *mut i32) -> i32, // [0x30] 恒0
    pub f7: unsafe extern "system" fn(P, *mut i32) -> i32, // [0x38] 恒0
    pub f8: unsafe extern "system" fn(P, Pw) -> i32, // [0x40]
    pub f9: unsafe extern "system" fn(P, Pw) -> i32, // [0x48]
}

// 偏移断言：游戏按固定偏移取槽位，改动必须在这里编译期报错。
const _: () = {
    assert!(std::mem::offset_of!(LoginVtbl, f10) == 0x50);
    assert!(std::mem::offset_of!(LoginVtbl, f24) == 0xC0);
    assert!(std::mem::offset_of!(InfoVtbl, f3) == 0x18);
    assert!(std::mem::size_of::<LoginVtbl>() == 26 * 8);
    assert!(std::mem::size_of::<InfoVtbl>() == 10 * 8);
};

static LOGIN_VTBL: LoginVtbl = LoginVtbl {
    q0,
    q1,
    q2,
    f3: f3_ok,
    f4: f4_show_login_dialog,
    f5: f5_close,
    f6: f6_move,
    f7: f7_logout,
    f8: f8_do_login,
    f9: f9_ok,
    f10: f10_get_ticket,
    f11: f11_ok,
    f12: f12_log_bstr,
    f13: f13_log3,
    f14: f14_ok,
    f15: f15_empty_bstr,
    f16: f16_ok,
    f17: f17_ok,
    f18: f18_ok,
    f19: f19_ok,
    f20: f20_set_client_type,
    f21: f21_ok,
    f22: f22_auth_code_login,
    f23: f23_session_login_game,
    f24: f24_get_ticket_for_appid,
    f25: f25_ok,
};

static INFO_VTBL: InfoVtbl = InfoVtbl {
    i0,
    i1,
    i2,
    f3: i3_get_command_line,
    f4: i4_ok,
    f5: i5_ok,
    f6: i6_ok,
    f7: i7_ok,
    f8: i8_ok,
    f9: i9_ok,
};

/// COM 对象头：首字段即虚表指针。
#[repr(C)]
pub struct LoginObj {
    pub vtbl: *const LoginVtbl,
}
#[repr(C)]
pub struct InfoObj {
    pub vtbl: *const InfoVtbl,
}
unsafe impl Sync for LoginObj {}
unsafe impl Sync for InfoObj {}

static LOGIN_OBJ: LoginObj = LoginObj { vtbl: &LOGIN_VTBL };
static INFO_OBJ: InfoObj = InfoObj { vtbl: &INFO_VTBL };

pub fn login_obj() -> P {
    &LOGIN_OBJ as *const LoginObj as P
}
pub fn info_obj() -> P {
    &INFO_OBJ as *const InfoObj as P
}

/// 命中的 IID 对应哪个对象。
pub enum Iid {
    Login,
    Info,
}

/// IID 命中判定：未知 IID 返回 `None`（调用方据此返回失败码）。
pub fn iid_kind(iid: &str) -> Option<Iid> {
    if IID_LOGIN.iter().any(|x| x.eq_ignore_ascii_case(iid)) {
        return Some(Iid::Login);
    }
    if IID_INFO.eq_ignore_ascii_case(iid) {
        return Some(Iid::Info);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iid_matching() {
        assert!(matches!(
            iid_kind("7B06DAD6-6832-4455-AFC6-6C8BE902534B"),
            Some(Iid::Login)
        ));
        assert!(matches!(
            iid_kind("7b06dad6-6832-4455-afc6-6c8be902534b"),
            Some(Iid::Login)
        ));
        assert!(matches!(
            iid_kind("D09EE9A2-8C44-42D6-9FE2-E34727BBEA10"),
            Some(Iid::Login)
        ));
        assert!(matches!(
            iid_kind("2B6523B0-9D08-424B-94CC-6BA173C2DF26"),
            Some(Iid::Info)
        ));
        assert!(iid_kind("00000000-0000-0000-0000-000000000000").is_none());
    }

    #[test]
    fn get_ticket_reads_env_and_fails_when_missing() {
        let _g = state::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state::clear_delivery_env();
        let mut t: Pw = std::ptr::null_mut();
        let mut s: Pw = std::ptr::null_mut();
        assert_eq!(fill_ticket(&mut t, &mut s), -1);
        assert!(t.is_null() && s.is_null(), "失败时出参必须置 NULL");

        std::env::set_var(ENV_TICKET, "ULS21-TESTTICKET");
        std::env::set_var(ENV_SNDAID, "1234567890");
        assert_eq!(fill_ticket(&mut t, &mut s), 0);
        assert_eq!(win::read_wstr(t, 64), "ULS21-TESTTICKET");
        assert_eq!(win::read_wstr(s, 64), "1234567890");
        state::clear_delivery_env();
    }

    #[test]
    fn info_get_command_line() {
        let _g = state::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state::clear_delivery_env();
        let mut base: Pw = std::ptr::null_mut();
        let mut area: i32 = 0;
        assert_eq!(
            unsafe { i3_get_command_line(std::ptr::null_mut(), &mut base, &mut area) },
            -1
        );
        std::env::set_var(ENV_BASE, "-AppID=100001900 -AreaID=7");
        std::env::set_var(ENV_AREAID, "7");
        assert_eq!(
            unsafe { i3_get_command_line(std::ptr::null_mut(), &mut base, &mut area) },
            0
        );
        assert_eq!(area, 7);
        assert!(win::read_wstr(base, 128).contains("-AreaID=7"));
        state::clear_delivery_env();
    }

    #[test]
    fn logout_clears_env_only() {
        let _g = state::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var(ENV_TICKET, "T");
        std::env::set_var(ENV_SNDAID, "S");
        std::env::set_var(ENV_AREAID, "7");
        std::env::set_var(ENV_BASE, "B");
        assert_eq!(unsafe { f7_logout(std::ptr::null_mut()) }, 0);
        assert!(!state::delivery_ready());
    }

    #[test]
    fn show_login_dialog_pending_semantics() {
        let call = || unsafe {
            f4_show_login_dialog(
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        let _g = state::ENV_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state::clear_delivery_env();
        assert_eq!(call(), 0x8000_000Au32 as i32);
        std::env::set_var(ENV_TICKET, "T");
        std::env::set_var(ENV_SNDAID, "S");
        std::env::set_var(ENV_AREAID, "7");
        std::env::set_var(ENV_BASE, "B");
        assert_eq!(call(), 0);
        state::clear_delivery_env();
    }
}
