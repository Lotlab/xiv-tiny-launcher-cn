//! 附属请求：失败不阻断；仅人脸验证强制阻断。

use proto::consts::*;
use proto::log;
use proto::params;
use proto::resp;

use crate::ctx::{Ctx, LoginTicket};
use crate::error::{Error, Result};
use crate::http::{self, Timeout};

/// 登录前附属请求，先于拿 guid 发出。
pub fn pre_login() {
    let p = params::path_get_message_file(LOGIN_APP_ID, LOGIN_AREA_ID);
    spawn("登录前 getMessageFile", HOST_BSC, p);
    spawn("agreement", HOST_UTILITY, params::path_agreement());
}

/// 登录后附属请求（getPromotionInfo / getLoginUserInfo 用登录后缀，
/// getSystemConfig 用无 group 后缀）。
pub fn post_login(ctx: &Ctx, login: &LoginTicket, area_id: &str) -> Result<()> {
    let p = params::path_get_message_file(GAME_APP_ID, area_id);
    spawn("登录后 getMessageFile", HOST_BSC, p);
    let s = proto::params::Suffix::login(&ctx.device, &ctx.run_time_id);
    let promotion = params::path_get_promotion_info(&s, &login.tgt);
    let user_info = params::path_get_login_user_info(&s, &login.tgt);
    spawn("getPromotionInfo", HOST_CAS, promotion);
    spawn("getLoginUserInfo", HOST_CAS, user_info);
    let cfg = params::path_get_system_config(&proto::params::Suffix::login_no_group(
        &ctx.device,
        &ctx.run_time_id,
    ));
    spawn("getSystemConfig", HOST_N_CAS, cfg);

    let fv = params::path_face_verify_init(&ctx.device, &login.tgt);
    let r = http::get(HOST_GFC, &fv, Timeout::Auth)?;
    if r.status != 200 {
        return Ok(());
    }
    let json = match r.json() {
        Ok(j) => j,
        Err(_) => {
            return Ok(());
        }
    };
    let rc = resp::result_code(&json);
    let open_face = resp::open_face(&json).unwrap_or_default();
    if open_face == "1" {
        log::warn("需要人脸验证，请在官方客户端完成验证后重试");
        return Err(Error::msg("需要人脸验证，请在官方客户端完成验证后重试"));
    }
    if rc != Some(0) {
        log::debug("人脸验证接口返回非成功，已忽略");
    }
    Ok(())
}

/// 后台 best-effort 请求，失败仅记 debug。
fn spawn(tag: &'static str, host: &'static str, path: String) {
    std::thread::spawn(move || {
        if let Err(e) = http::get(host, &path, Timeout::Download) {
            log::debug(&format!("附属请求[{tag}]失败：{e}"));
        }
    });
}
