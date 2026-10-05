//! 层 1：[`Api`] —— 一个方法 = 一个服务端接口，只做「发请求 + 解析 + 记状态」。
//!
//! 登录链需要的服务端值（`guid` / `codeKey` / `tgt` / `authorization`）留在对象内部，
//! 调用方不拼 query、也不回带这些值。流程在层 2，见 [`crate::Flow`]。

use crate::consts::*;

use crate::endpoint::{
    Agreement, App, CancelPush, CodeKeyLogin, Endpoint, FaceVerifyInit, FastInLogin, GetCodeKey,
    GetGuid, LoginUserInfo, Promotion, PushLogin, SendPush, ServerJson, SsoAuthorization, SsoLogin,
    Suffix, SystemConfig,
};
use crate::error::{Error, Result};
use crate::resp;
use crate::server::{self, ServerTable};
use crate::tickets::GameTicket;
use crate::transport::{fetch, Identity, Resp};

/// SDO 接口客户端。
///
/// 只持**会话身份**与**登录链状态**；应用口径（哪个 App）由调用方按次传入。
pub struct Api {
    identity: Identity,
    run_time_id: String,
    guid: Option<String>,
    tgt: Option<String>,
    new_keep_key: Option<String>,
}

impl Api {
    /// 会话身份：设备快照 + 本进程 `runTimeId`。
    pub fn new(identity: Identity, run_time_id: impl Into<String>) -> Api {
        Api {
            identity,
            run_time_id: run_time_id.into(),
            guid: None,
            tgt: None,
            new_keep_key: None,
        }
    }

    // ── 会话状态的两个读口（其余状态完全内部）──

    /// 登录链当前的 `guid`（`getGuid.json` 拿到的）。
    /// `fastInLogin` 可能没带，层 2 据此补一次 [`Api::get_guid`]。
    pub fn guid(&self) -> Option<&str> {
        self.guid.as_deref()
    }

    /// 服务端这次回的续登凭据；落盘由 EXE 做。
    pub fn new_keep_login_key(&self) -> Option<&str> {
        self.new_keep_key.as_deref()
    }

    // ── 内部：拼后缀与发请求 ──

    fn suffix(&self, app: &App) -> Suffix {
        Suffix::login(&self.identity, &self.run_time_id, app)
    }

    fn suffix_no_group(&self, app: &App) -> Suffix {
        Suffix::login_no_group(&self.identity, &self.run_time_id, app)
    }

    fn get<E: Endpoint>(&self, ep: &E) -> Result<Resp> {
        fetch(E::HOST, &ep.path(), E::TIMEOUT)
    }

    /// 登录成功响应里要记的状态。
    fn absorb_login(&mut self, json: &serde_json::Value) {
        if let Some(tgt) = resp::data_str(json, "tgt").filter(|t| !t.is_empty()) {
            self.tgt = Some(tgt);
        }
        if let Some(k) = resp::data_str(json, "keepLoginKey").filter(|k| !k.is_empty()) {
            self.new_keep_key = Some(k);
        }
    }

    // ── getGuid.json ──

    /// `getGuid.json`：取一次会话 `guid`。
    pub fn get_guid(&mut self, app: &App) -> Result<String> {
        let r = self.get(&GetGuid::new(self.suffix(app)))?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("登录服务繁忙（HTTP {}），请重试", r.status),
            ));
        }
        let json = r.json()?;
        let guid = resp::data_str(&json, "guid")
            .filter(|g| !g.is_empty())
            .ok_or_else(|| Error::parse("登录服务返回异常，请重试"))?;
        self.guid = Some(guid.clone());
        Ok(guid)
    }

    // ── getCodeKey.json ──

    /// `getCodeKey.json`：取二维码图片与响应头 `CODEKEY` 里的 `codeKey`。
    pub fn get_code_key(&self, app: &App) -> Result<QrCode> {
        let r = self.get(&GetCodeKey::new(self.suffix(app)))?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("二维码服务繁忙（HTTP {}），请重试", r.status),
            ));
        }
        if !resp::is_png(&r.body) {
            return Err(Error::parse("二维码响应异常，请重试"));
        }
        let code_key = resp::extract_codekey(r.header_values("set-cookie"))
            .ok_or_else(|| Error::parse("二维码响应异常，请重试"))?;
        Ok(QrCode {
            png: r.body,
            code_key,
        })
    }

    // ── codeKeyLogin.json ──

    /// `codeKeyLogin.json`：扫码轮询。
    /// `Err` = 传输/HTTP/解析失败，调用方计入一次尝试继续等。
    pub fn code_key_login(&mut self, app: &App, code_key: &str, keep_flag: i32) -> Result<Poll> {
        let guid = self
            .guid
            .clone()
            .ok_or_else(|| Error::state("尚未取得 guid（先调 get_guid）"))?;
        let ep = CodeKeyLogin::new(self.suffix(app), code_key, guid, keep_flag);
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(r.status, "扫码轮询失败，请重试")
                .with_detail(format!("扫码轮询状态码 {}", r.status)));
        }
        let json = r.json()?;
        if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            self.absorb_login(&json);
            return Ok(Poll::Ok);
        }
        if resp::return_code(&json) == Some(RC_QR_NOT_SCANNED) {
            return Ok(Poll::Pending);
        }
        Ok(Poll::Failed(resp::fail_reason_text(&json)))
    }

    // ── fastInLogin ──

    /// `fastInLogin`：用续登凭据自动登录。成功时 `tgt` 记入状态，`guid` 只有响应带了才有。
    pub fn fast_in_login(&mut self, app: &App, keep_login_key: &str) -> Result<FastLogin> {
        let ep = FastInLogin::new(self.suffix_no_group(app), keep_login_key);
        let json = match self.get(&ep) {
            Ok(r) if r.status == 200 => r.json()?,
            Ok(r) => return Err(Error::http(r.status, "自动登录服务繁忙，请重试")),
            Err(e) => return Err(Error::transport(format!("自动登录请求失败: {}", e.log_text()))),
        };
        if !resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            return Ok(FastLogin::Rejected(resp::fail_reason_text(&json)));
        }
        self.absorb_login(&json);
        if let Some(g) = resp::data_str(&json, "guid").filter(|g| !g.is_empty() && g != "null") {
            self.guid = Some(g);
        }
        Ok(FastLogin::Ok)
    }

    // ── cancelPushMessageLogin.json / sendPushMessage.json / pushMessageLogin.json ──

    /// `cancelPushMessageLogin.json`：取消上一次待确认的手机推送。
    /// 空 key 必回 `-10242301`，结果无意义。
    pub fn cancel_push(&self, app: &App) -> Result<()> {
        let guid = self
            .guid
            .clone()
            .ok_or_else(|| Error::state("尚未取得 guid（先调 get_guid）"))?;
        let _ = self.get(&CancelPush::new(self.suffix(app), guid))?;
        Ok(())
    }

    /// `sendPushMessage.json`：向账号发一次手机确认推送，返回会话标识。
    pub fn send_push(&self, app: &App, account: &str) -> Result<String> {
        let ep = SendPush::new(self.suffix(app), account);
        let json = match self.get(&ep) {
            Ok(r) if r.status == 200 => r.json()?,
            Ok(r) => return Err(Error::http(r.status, format!("手机推送 HTTP {}", r.status))),
            Err(e) => return Err(Error::transport(format!("手机推送请求失败: {}", e.log_text()))),
        };
        if resp::return_code(&json) != Some(RC_OK) {
            return Err(Error::rejected(resp::fail_reason_text(&json)));
        }
        let session_key = resp::data_str(&json, "pushMsgSessionKey").unwrap_or_default();
        if session_key.is_empty() {
            return Err(Error::rejected("手机推送未返回会话标识"));
        }
        Ok(session_key)
    }

    /// `pushMessageLogin.json`：手机确认轮询。
    /// `Err` = 传输/HTTP/解析失败。
    pub fn push_message_login(&mut self, app: &App, session_key: &str) -> Result<Poll> {
        let guid = self
            .guid
            .clone()
            .ok_or_else(|| Error::state("尚未取得 guid（先调 get_guid）"))?;
        let ep = PushLogin::new(self.suffix(app), session_key, guid);
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(r.status, "手机确认轮询失败，请重试")
                .with_detail(format!("手机确认轮询状态码 {}", r.status)));
        }
        let json = r.json()?;
        if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            self.absorb_login(&json);
            return Ok(Poll::Ok);
        }
        if resp::return_code(&json) == Some(RC_PUSH_NOT_CONFIRMED) {
            return Ok(Poll::Pending);
        }
        Ok(Poll::Failed(resp::fail_reason_text(&json)))
    }

    // ── getSsoAuthorization / ssoAuthorizationLogin ──

    /// `getSsoAuthorization`：换票第一步 —— 以 `from` 的版本、向 `to` 申请授权，
    /// 返回 `authorization`。
    pub fn sso_authorization(&self, from: &App, to: &App) -> Result<String> {
        let tgt = self
            .tgt
            .clone()
            .ok_or_else(|| Error::state("尚未取得 tgt（先完成登录）"))?;
        let guid = self
            .guid
            .clone()
            .ok_or_else(|| Error::state("尚未取得 guid（先调 get_guid）"))?;
        let s = Suffix::for_sso_authorization(&self.identity, &self.run_time_id, from, to);
        let r = self.get(&SsoAuthorization::new(s, tgt, guid))?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("换票服务不可用（HTTP {}），请稍后重试", r.status),
            ));
        }
        let json = r.json()?;
        resp::data_str(&json, "authorization")
            .filter(|a| !a.is_empty())
            .ok_or_else(|| Error::rejected(format!("换票失败：{}", resp::fail_reason_text(&json))))
    }

    /// `ssoAuthorizationLogin`：换票第二步 —— 用上一步的授权换 T1。
    pub fn sso_authorization_login(&self, to: &App, authorization: &str) -> Result<GameTicket> {
        let s = Suffix::for_sso_login(&self.identity, &self.run_time_id, to);
        let r = self.get(&SsoLogin::new(s, authorization))?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("换票服务不可用（HTTP {}），请稍后重试", r.status),
            ));
        }
        let json = r.json()?;
        // 实测 `ssoAuthorizationLogin` 会返回 `tgt`，因此继续要求它（只作为成功标志，不消费）。
        if !resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            return Err(Error::rejected(format!(
                "换票失败：{}",
                resp::fail_reason_text(&json)
            )));
        }
        Ok(GameTicket {
            ticket: resp::data_str(&json, "ticket").unwrap_or_default(),
            snda_id: resp::data_str(&json, "sndaId").unwrap_or_default(),
        })
    }

    // ── server.json ──

    /// 区服表（`v3launcher/server/{appId}/8847/server.json`）：返回解析后的表 +
    /// 服务端原文（缓存策略是调用方的事）。取 [`GAME_APP_ID`](crate::GAME_APP_ID)
    /// —— 拉表早于选区确定，给不出完整的 `App`。
    pub fn server_json(&self, app_id: i32) -> Result<FetchedTable> {
        let ep = ServerJson::new(app_id, proto::clock::now_millis());
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(r.status, "区服列表获取失败，请重试")
                .with_detail(format!("区服接口状态码 {}", r.status)));
        }
        let table = server::parse_server_json(&r.text())?;
        Ok(FetchedTable {
            table,
            raw: r.body,
        })
    }

    // ── faceVerify/init ──

    /// `faceVerify/init`：人脸验证初始化。顶层字段是 `resultCode` 而非 `return_code`；
    /// 要不要因此阻断启动是层 2 的规则。
    pub fn face_verify_init(&self, app: &App) -> Result<FaceVerify> {
        let tgt = self
            .tgt
            .as_deref()
            .ok_or_else(|| Error::state("尚未取得 tgt（先完成登录）"))?;
        let ep = FaceVerifyInit::new(
            self.identity.device_id.clone(),
            app.app_id,
            app.area_id,
            app.product_version,
            tgt,
        );
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("人脸验证服务繁忙（HTTP {}）", r.status),
            ));
        }
        let json = r.json()?;
        Ok(FaceVerify {
            result_code: resp::result_code(&json),
            open_face: resp::open_face(&json),
        })
    }

    // ── 附属请求：层 1 构造好（自持数据、不借 `Api`），层 2 决定何时/在哪个线程发 ──

    /// `agreement/user`：用户协议。
    pub fn agreement_request(&self, app_id: i32) -> Request {
        Request::new(Agreement::new(app_id))
    }

    /// `getPromotionInfo.json`：活动信息（用 `tgt`）。
    pub fn promotion_request(&self, app: &App) -> Request {
        Request::new(Promotion::new(self.suffix(app), self.tgt.clone().unwrap_or_default()))
    }

    /// `getLoginUserInfo.json`：登录用户信息（用 `tgt`）。
    pub fn login_user_info_request(&self, app: &App) -> Request {
        Request::new(LoginUserInfo::new(
            self.suffix(app),
            self.tgt.clone().unwrap_or_default(),
        ))
    }

    /// `getSystemConfig`：服务端系统配置（该模板无 `groupId`）。
    pub fn system_config_request(&self, app: &App) -> Request {
        Request::new(SystemConfig::new(self.suffix_no_group(app)))
    }

    // ── 自检 ──

    /// 渲染关键端点的 `path?query`（不发送），给 `--self-check` 断言参数口径用。
    /// `app` 是登录链口径，`target` 是换入的游戏应用（含选区）。
    pub fn probe_paths(&self, app: &App, target: &App) -> ProbePaths {
        let id = &self.identity;
        let rtid = &self.run_time_id;
        ProbePaths {
            fast_in_login: FastInLogin::new(
                Suffix::login_no_group(id, rtid, app),
                "<keepLoginKey>",
            )
            .path(),
            sso_authorization: SsoAuthorization::new(
                Suffix::for_sso_authorization(id, rtid, app, target),
                "<tgt0>",
                "<guid0>",
            )
            .path(),
            sso_authorization_login: SsoLogin::new(
                Suffix::for_sso_login(id, rtid, target),
                "<authorization>",
            )
            .path(),
        }
    }
}

/// `codeKeyLogin` / `pushMessageLogin` 的响应。
///
/// 两条轮询链形状相同所以共用；各自的返回码（`-10515805` / `-10516808`）
/// 在层 1 内部消化，层 2 不需要认识它们。
pub enum Poll {
    /// 成功：票据已记入状态。
    Ok,
    /// 还没好（未扫码 / 未确认）：继续等。
    Pending,
    /// 服务端给了其它错误码（内容为展示用原文）。
    Failed(String),
}

/// `fastInLogin`（自动登录）的响应。
pub enum FastLogin {
    /// 成功：`tgt` 已记入状态；`guid` 可能没带，见 [`Api::guid`]。
    Ok,
    /// 服务端明确拒绝该 `keepLoginKey`：层 2 应清除本地凭据并转二维码。
    Rejected(String),
}

/// `faceVerify/init` 的响应。
pub struct FaceVerify {
    pub result_code: Option<i64>,
    pub open_face: Option<String>,
}

impl FaceVerify {
    /// 服务端是否要求人脸验证（`openFace == "1"`）。
    pub fn is_required(&self) -> bool {
        self.open_face.as_deref() == Some("1")
    }
}

/// `getCodeKey` 的响应：二维码图片 + 响应头里的 `codeKey`（下次轮询要回带）。
pub struct QrCode {
    pub png: Vec<u8>,
    pub code_key: String,
}

/// `server.json` 的响应：解析后的表 + 服务端原文。
pub struct FetchedTable {
    pub table: ServerTable,
    /// 服务端原文：调用方（EXE）拿它写本地备份。
    pub raw: Vec<u8>,
}

/// 一个已构造好的请求：自持全部数据（不借 [`Api`]），可 move 进线程后发送。
pub struct Request(Box<dyn FnOnce() -> Result<()> + Send>);

impl Request {
    fn new<E: Endpoint + Send + 'static>(ep: E) -> Request {
        Request(Box::new(move || {
            fetch(E::HOST, &ep.path(), E::TIMEOUT).map(|_| ())
        }))
    }

    /// 发送。
    pub fn send(self) -> Result<()> {
        (self.0)()
    }
}

/// 自检用的关键端点 `path?query`。
pub struct ProbePaths {
    pub fast_in_login: String,
    pub sso_authorization: String,
    pub sso_authorization_login: String,
}
