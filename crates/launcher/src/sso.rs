//! 换票：每次启动必做，换票前不起游戏。

use proto::consts::*;
use proto::log;
use proto::params::{self, Suffix};
use proto::resp;

use crate::ctx::{Ctx, GameTicket, LoginTicket};
use crate::error::{Error, Result};
use crate::http::{self, Timeout};

/// 两步换出游戏票据，失败返回 Err。
pub fn exchange(ctx: &Ctx, login: &LoginTicket, area_id: &str) -> Result<GameTicket> {
    let s1 = Suffix::for_sso_authorization(&ctx.device, &ctx.run_time_id, area_id);
    let p1 = params::path_get_sso_authorization(&s1, &login.tgt, &login.guid);
    let r1 = http::get(HOST_CAS, &p1, Timeout::Auth)?;
    if r1.status != 200 {
        return Err(Error::msg(format!(
            "换票服务不可用（HTTP {}），请稍后重试",
            r1.status
        )));
    }
    let j1 = r1.json()?;
    let auth = resp::data_str(&j1, "authorization")
        .filter(|a| !a.is_empty())
        .ok_or_else(|| Error::msg(format!("换票失败：{}", resp::fail_reason_text(&j1))))?;

    let s2 = Suffix::for_sso_login(&ctx.device, &ctx.run_time_id, area_id);
    let p2 = params::path_sso_authorization_login(&s2, &auth);
    let r2 = http::get(HOST_CAS, &p2, Timeout::Auth)?;
    if r2.status != 200 {
        return Err(Error::msg(format!(
            "换票服务不可用（HTTP {}），请稍后重试",
            r2.status
        )));
    }
    let j2 = r2.json()?;
    if !resp::is_success(&j2, &["ticket", "sndaId", "tgt"]) {
        return Err(Error::msg(format!(
            "换票失败：{}",
            resp::fail_reason_text(&j2)
        )));
    }
    let game = GameTicket {
        ticket: resp::data_str(&j2, "ticket").unwrap_or_default(),
        snda_id: resp::data_str(&j2, "sndaId").unwrap_or_default(),
    };
    if game.ticket == login.ticket {
        log::debug("换票结果与登录票相同");
    }
    Ok(game)
}
