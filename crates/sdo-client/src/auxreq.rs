//! 附属请求：全 fire-and-forget，失败不阻断（人脸验证除外，见 [`Client::check_face_verify`]）。
//! `tgt0` 系登录应用。

use proto::log;

use crate::client::{Client, Result, fetch};
use crate::endpoint::{
    Agreement, Endpoint, FaceVerify, GameApp, LoginUserInfo, MessageFile, Promotion, Suffix,
    SystemConfig,
};
use crate::resp;

impl Client {
    fn aux_suffix(&self) -> Suffix {
        Suffix::login(self.identity(), self.run_time_id(), self.login_app())
    }

    fn aux_no_group_suffix(&self) -> Suffix {
        Suffix::login_no_group(self.identity(), self.run_time_id(), self.login_app())
    }

    /// 登录前附属请求，先于拿 guid 发出。
    pub fn pre_login(&self) {
        let app = self.login_app();
        spawn(
            "登录前 getMessageFile",
            MessageFile::new(app.app_id.clone(), app.area.clone()),
        );
        spawn("agreement", Agreement::new(app.app_id.clone()));
    }

    /// 登录后附属请求中可后台发出的部分（`getMessageFile/promotion/userInfo/systemConfig`）。
    /// `game` 按次传入（选区 + 游戏应用口径）。
    pub fn post_login_fire_and_forget(&self, tgt0: &str, game: &GameApp) {
        spawn(
            "登录后 getMessageFile",
            MessageFile::new(game.app_id.clone(), game.area_id.clone()),
        );
        let s = self.aux_suffix();
        spawn("getPromotionInfo", Promotion::new(s.clone(), tgt0));
        spawn("getLoginUserInfo", LoginUserInfo::new(s, tgt0));
        spawn(
            "getSystemConfig",
            SystemConfig::new(self.aux_no_group_suffix()),
        );
    }

    /// 人脸验证（同步）：成功 `resultCode == 0 && openFace == "0"`（注意顶层非 `return_code`）；
    /// `openFace == "1"` 终止；其余非成功只记日志，不阻断。
    pub fn check_face_verify(&self, tgt0: &str) -> Result<()> {
        let app = self.login_app();
        let ep = FaceVerify::new(
            self.identity().device_id.clone(),
            app.app_id.clone(),
            app.area.clone(),
            app.product_version.clone(),
            tgt0,
        );
        let r = self.get(&ep)?;
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
            return Err("需要人脸验证，请在官方客户端完成验证后重试".to_string());
        }
        if rc != Some(0) {
            log::debug("人脸验证接口返回非成功，已忽略");
        }
        Ok(())
    }
}

/// 后台 best-effort 请求，失败仅记 debug。
fn spawn<E: Endpoint + Send + 'static>(tag: &'static str, ep: E) {
    std::thread::spawn(move || {
        if let Err(e) = fetch(E::HOST, &ep.path(), E::TIMEOUT) {
            log::debug(&format!("附属请求[{tag}]失败：{e}"));
        }
    });
}
