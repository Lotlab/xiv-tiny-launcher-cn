//! 换票：每次启动必做，换票前不起游戏。
//!
//! - 输入：T0（`ticket0/tgt0/guid0`；QR = `getGuid.guid` + 轮询 `tgt`；
//!   fast = 现取 `guid0` + 响应 `tgt`；push = 最后一轮 `guid` + 响应 `tgt`）。
//! - 输出：T1（新 `ticket1/sndaId1`，不更新 `tgt`，无 `tgt1`）；交接给游戏的必须是 T1，
//!   附属请求用 `tgt0`（`791000814` 系）。任一步失败调用方丢 T0（**不删 key**）回 QR。

use proto::log;

use crate::client::{Client, Result};
use crate::endpoint::{GameApp, SsoAuthorization, SsoLogin, Suffix};
use crate::resp;
use crate::tickets::{GameTicket, LoginTicket};

impl Client {
    /// 两步换出游戏票据，失败返回 Err。`game` 按次传入（选区 + 游戏应用口径）。
    pub fn exchange(&self, login: &LoginTicket, game: &GameApp) -> Result<GameTicket> {
        let s1 = Suffix::for_sso_authorization(self.identity(), self.run_time_id(), game);
        let r1 = self.get(&SsoAuthorization::new(s1, &login.tgt, &login.guid))?;
        if r1.status != 200 {
            return Err(format!(
                "换票服务不可用（HTTP {}），请稍后重试",
                r1.status
            ));
        }
        let j1 = r1.json()?;
        let auth = resp::data_str(&j1, "authorization")
            .filter(|a| !a.is_empty())
            .ok_or_else(|| format!("换票失败：{}", resp::fail_reason_text(&j1)))?;

        let s2 = Suffix::for_sso_login(self.identity(), self.run_time_id(), game);
        let r2 = self.get(&SsoLogin::new(s2, &auth))?;
        if r2.status != 200 {
            return Err(format!(
                "换票服务不可用（HTTP {}），请稍后重试",
                r2.status
            ));
        }
        let j2 = r2.json()?;
        if !resp::is_success(&j2, &["ticket", "sndaId", "tgt"]) {
            return Err(format!("换票失败：{}", resp::fail_reason_text(&j2)));
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
}
