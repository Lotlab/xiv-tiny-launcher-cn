//! 层 2：[`Flow`] —— FFXIV 启动器的流程：登录链、换票、人脸验证、附属请求。
//!
//! 循环、回退与业务规则都在这里。不碰文件、UI、进程：UI 通过 [`Ui`] 反向调用，
//! 落盘与起进程由 EXE 做。

use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use proto::consts::*;
use proto::log;

use crate::api::{Api, FastLogin, Poll, Request};
use crate::endpoint::App;
use crate::error::{Error, Kind, Result};
use crate::tickets::GameTicket;
use crate::ui::{Action, Note, Phase, Ui, Wait};

/// 登录模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// 二维码扫码。
    Qr,
    /// 手机 App 确认。
    Push,
    /// 有续登凭据就先试自动登录，失败回二维码。
    Auto,
}

/// 对 `device.json` 里 `keepLoginKey` 的处理决定。落盘由 EXE 做。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepKey {
    Keep,
    /// 服务端给了新的：EXE 覆盖落盘。
    Replace(String),
    /// 服务端明确拒绝：EXE 删除。
    Clear,
}

/// 流程参数。默认值取自 `proto::consts`，EXE 按 CLI 覆盖。
#[derive(Debug, Clone)]
pub struct Policy {
    pub mode: Mode,
    /// `push` 模式用的账号。
    pub account: Option<String>,
    /// 单张二维码 / 单次手机确认的有效秒数。
    pub code_timeout_secs: u64,
    /// 单张码最多轮询次数。
    pub max_attempts: u32,
    /// 轮询间隔范围（抖动）。
    pub poll_min_ms: u64,
    pub poll_max_ms: u64,
    /// 单码连续换码上限。
    pub max_code_rounds: u32,
    /// 换票连续失败后重新登录的轮数上限。
    pub max_login_rounds: u32,
    /// 手机推送发送重试次数与间隔。
    pub push_send_tries: u32,
    pub push_send_retry_ms: u64,
    /// 退出前等待附属请求的总预算。
    pub aux_wait_ms: u64,
}

impl Default for Policy {
    fn default() -> Policy {
        Policy {
            mode: Mode::Auto,
            account: None,
            code_timeout_secs: DEFAULT_QR_TIMEOUT_SECS,
            max_attempts: DEFAULT_QR_MAX_ATTEMPTS,
            poll_min_ms: DEFAULT_POLL_MIN_MS,
            poll_max_ms: DEFAULT_POLL_MAX_MS,
            max_code_rounds: MAX_QR_CODE_ROUNDS,
            max_login_rounds: MAX_LOGIN_ROUNDS,
            push_send_tries: PUSH_SEND_MAX_TRIES,
            push_send_retry_ms: PUSH_SEND_RETRY_WAIT_MS,
            aux_wait_ms: AUX_WAIT_BUDGET_MS,
        }
    }
}

/// FFXIV 启动器的流程。
pub struct Flow {
    policy: Policy,
    /// 登录链的应用口径（"当前 App"）。
    app: App,
    /// 换入的游戏应用（含本次选区）。
    target: App,
    /// 后台附属请求的句柄；退出前统一等一等。
    pending: Vec<JoinHandle<()>>,
    keep_key: KeepKey,
}

impl Flow {
    /// `app` 是登录链的应用口径，`target` 是换入的游戏应用（含本次选区）。
    pub fn new(policy: Policy, app: App, target: App) -> Flow {
        Flow {
            policy,
            app,
            target,
            pending: Vec::new(),
            keep_key: KeepKey::Keep,
        }
    }

    /// `run` 返回后读；成功失败都要落盘。
    pub fn keep_key(&self) -> &KeepKey {
        &self.keep_key
    }

    /// 一次完整会话：登录 → 换票（失败回登录）→ 登录后附属请求 → 人脸验证。
    /// `stored_keep_key` 是 EXE 从 `device.json` 读出的续登凭据。
    pub fn run(
        &mut self,
        api: &mut Api,
        ui: &mut dyn Ui,
        stored_keep_key: Option<&str>,
    ) -> Result<GameTicket> {
        self.keep_key = KeepKey::Keep;
        self.pre_login(api);

        let mut round = 0u32;
        let game_ticket = loop {
            round += 1;
            if round > self.policy.max_login_rounds {
                return Err(Error::rejected(format!(
                    "换票连续 {} 轮失败，已终止",
                    self.policy.max_login_rounds
                )));
            }
            match self.login(api, ui, stored_keep_key) {
                Ok(()) => {}
                Err(e) if e.is_quit() => return Err(e),
                Err(e) => return Err(e),
            }
            match self.exchange(api) {
                Ok(t) => break t,
                Err(e) => {
                    log::info(&format!("换票失败：{}", e.log_text()));
                    ui.note(Note::ExchangeFailed(e.user().to_string()));
                }
            }
        };

        self.post_login(api);
        self.check_face_verify(api)?;
        Ok(game_ticket)
    }

    /// 退出前等一等后台附属请求；超时未完成的只写日志后放弃。
    pub fn wait_pending(&mut self, budget: Duration) {
        let handles = std::mem::take(&mut self.pending);
        if handles.is_empty() {
            return;
        }
        let deadline = Instant::now() + budget;
        let mut unfinished = 0usize;
        for h in handles {
            // 标准库没有带超时的 join，只能轮询 is_finished。
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            } else {
                unfinished += 1; // 句柄在此丢弃，不再等待。
            }
        }
        if unfinished > 0 {
            log::warn(&format!("附属请求仍有 {unfinished} 个未完成，已放弃等待"));
        }
    }

    // ── 登录 ──

    /// 按模式选登录链；手机链失败回退二维码。
    fn login(&mut self, api: &mut Api, ui: &mut dyn Ui, stored_keep_key: Option<&str>) -> Result<()> {
        match self.policy.mode {
            Mode::Qr => self.login_qr(api, ui),
            Mode::Push => {
                let account = self.policy.account.clone().unwrap_or_default();
                match self.login_push(api, ui, &account) {
                    Ok(()) => Ok(()),
                    Err(e) if e.is_quit() => Err(e),
                    Err(e) => {
                        log::warn(&format!(
                            "手机登录不可用（{}），转为二维码登录",
                            e.log_text()
                        ));
                        self.login_qr(api, ui)
                    }
                }
            }
            Mode::Auto => {
                let Some(key) = stored_keep_key else {
                    return self.login_qr(api, ui);
                };
                match api.fast_in_login(&self.app, key) {
                    Ok(FastLogin::Ok) => {
                        // 响应没带 guid 时补一次 getGuid（这是流程，不是接口）。
                        if api.guid().is_none() {
                            api.get_guid(&self.app)?;
                        }
                        self.absorb_keep_key(api);
                        Ok(())
                    }
                    Ok(FastLogin::Rejected(reason)) => {
                        log::warn(&format!(
                            "自动登录被拒绝（{reason}），清除本地登录信息并转二维码"
                        ));
                        self.keep_key = KeepKey::Clear;
                        self.login_qr(api, ui)
                    }
                    Err(e) => {
                        // 传输/HTTP/解析失败：key 可能仍然有效，保留。
                        log::warn(&format!("自动登录失败（{}），转为二维码登录", e.log_text()));
                        self.login_qr(api, ui)
                    }
                }
            }
        }
    }

    /// 二维码主流程：出码 → 轮询 → 换码循环。
    fn login_qr(&mut self, api: &mut Api, ui: &mut dyn Ui) -> Result<()> {
        api.get_guid(&self.app)?;
        let mut code_round = 0u32;
        let mut consecutive_errors = 0u32;

        loop {
            code_round += 1;
            if code_round > self.policy.max_code_rounds {
                return Err(Error::rejected(format!(
                    "连续更换 {} 张二维码仍未登录成功，终止",
                    self.policy.max_code_rounds
                )));
            }
            let code = api.get_code_key(&self.app)?;
            ui.show_code(&code.png, code_round);

            let started = Instant::now();
            let deadline = started + Duration::from_secs(self.policy.code_timeout_secs);
            let mut attempt = 0u32;
            let mut server_error = false;
            let refresh_reason: &'static str;

            loop {
                let left = deadline.saturating_duration_since(Instant::now()).as_secs();
                match ui.tick(Wait {
                    left_secs: left,
                    attempt,
                    phase: Phase::ScanCode,
                }) {
                    Action::Quit => return Err(Error::quit()),
                    Action::RefreshCode => {
                        refresh_reason = "手动取消";
                        break;
                    }
                    Action::Wait => {}
                }
                if Instant::now() >= deadline {
                    refresh_reason = "倒计时结束";
                    break;
                }

                let keep_flag = if ui.keep_login() {
                    KEEP_LOGIN_FLAG_CHECKED
                } else {
                    KEEP_LOGIN_FLAG_UNSET
                };
                match api.code_key_login(&self.app, &code.code_key, keep_flag) {
                    Ok(Poll::Ok) => {
                        self.absorb_keep_key(api);
                        ui.note(Note::Scanned);
                        return Ok(());
                    }
                    Ok(Poll::Pending) => attempt += 1,
                    Ok(Poll::Failed(text)) => {
                        log::debug(&format!("扫码返回错误：{text}"));
                        ui.note(Note::ServerError(text.clone()));
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            return Err(Error::rejected(format!(
                                "登录被拒绝（连续 {consecutive_errors} 次）：{text}"
                            )));
                        }
                        refresh_reason = "服务端返回错误";
                        server_error = true;
                        break;
                    }
                    Err(e) => {
                        attempt += 1;
                        log::debug(&format!("扫码轮询请求细节：{}", e.log_text()));
                    }
                }

                if attempt >= self.policy.max_attempts {
                    refresh_reason = "连续未成功";
                    break;
                }
                std::thread::sleep(Duration::from_millis(self.poll_delay_ms()));
            }

            if !server_error {
                consecutive_errors = 0;
            }
            let floor = Duration::from_millis(self.policy.poll_min_ms);
            if let Some(rest) = floor.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
            log::debug(&format!("第 {code_round} 张码结束（{refresh_reason}）"));
            ui.note(Note::CodeRefreshed(refresh_reason));
        }
    }

    /// 发送失败按参数重试；耗尽后由调用方转二维码。
    fn login_push(&mut self, api: &mut Api, ui: &mut dyn Ui, account: &str) -> Result<()> {
        if account.trim().is_empty() {
            return Err(Error::rejected("未提供 --account"));
        }
        let mut last = Error::rejected("手机推送发送失败");
        for round in 1..=self.policy.push_send_tries {
            api.get_guid(&self.app)?;
            let _ = api.cancel_push(&self.app);
            match api.send_push(&self.app, account) {
                Ok(session_key) => {
                    ui.note(Note::PushSent);
                    return self.poll_push(api, ui, &session_key);
                }
                Err(e) => {
                    log::warn(&format!(
                        "手机推送发送失败（第 {round}/{} 次）：{}",
                        self.policy.push_send_tries,
                        e.log_text()
                    ));
                    last = e;
                    std::thread::sleep(Duration::from_millis(self.policy.push_send_retry_ms));
                }
            }
        }
        Err(last)
    }

    fn poll_push(&mut self, api: &mut Api, ui: &mut dyn Ui, session_key: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(self.policy.code_timeout_secs);
        let mut attempt = 0u32;
        loop {
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            match ui.tick(Wait {
                left_secs: left,
                attempt,
                phase: Phase::ConfirmPush,
            }) {
                Action::Quit => return Err(Error::quit()),
                Action::RefreshCode => return Err(Error::rejected("已取消手机确认")),
                Action::Wait => {}
            }
            if Instant::now() >= deadline {
                return Err(Error::rejected(format!(
                    "手机确认 {}s 倒计时结束",
                    self.policy.code_timeout_secs
                )));
            }

            match api.push_message_login(&self.app, session_key) {
                Ok(Poll::Ok) => {
                    self.absorb_keep_key(api);
                    ui.note(Note::Scanned);
                    return Ok(());
                }
                Ok(Poll::Pending) => attempt += 1,
                Ok(Poll::Failed(reason)) => {
                    return Err(Error::rejected(format!("手机确认登录被服务端拒绝：{reason}")));
                }
                Err(e) => {
                    attempt += 1;
                    log::debug(&format!("手机确认轮询请求细节：{}", e.log_text()));
                }
            }

            if attempt >= self.policy.max_attempts {
                return Err(Error::rejected(format!(
                    "连续 {} 次未确认",
                    self.policy.max_attempts
                )));
            }
            std::thread::sleep(Duration::from_millis(self.poll_delay_ms()));
        }
    }

    /// 服务端回了新的续登凭据就覆盖本地决定。
    fn absorb_keep_key(&mut self, api: &Api) {
        if let Some(k) = api.new_keep_login_key() {
            self.keep_key = KeepKey::Replace(k.to_string());
        }
    }

    // ── 换票 ──

    /// 从当前 App 换到 `target`；失败语义由 [`Flow::run`] 决定。
    fn exchange(&self, api: &Api) -> Result<GameTicket> {
        let authorization = api.sso_authorization(&self.app, &self.target)?;
        api.sso_authorization_login(&self.target, &authorization)
    }

    // ── 人脸验证 ──

    /// 只有服务端明确要求（`openFace == "1"`）才阻断启动；HTTP 非 200 与解析失败
    /// 按"接口异常"忽略，传输失败仍然上抛。
    fn check_face_verify(&self, api: &Api) -> Result<()> {
        let face = match api.face_verify_init(&self.app) {
            Ok(f) => f,
            Err(e) if matches!(e.kind(), Kind::Http(_) | Kind::Parse) => {
                log::debug(&format!("人脸验证接口异常，已忽略：{}", e.log_text()));
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if face.is_required() {
            log::warn("需要人脸验证，请在官方客户端完成验证后重试");
            return Err(
                Error::rejected("需要人脸验证，请在官方客户端完成验证后重试")
                    .with_detail("openFace=1"),
            );
        }
        if face.result_code != Some(0) {
            log::debug(&format!(
                "人脸验证接口返回非成功，已忽略：resultCode={:?}",
                face.result_code
            ));
        }
        Ok(())
    }

    // ── 附属请求 ──

    /// 先于拿 guid 发出。
    fn pre_login(&mut self, api: &Api) {
        self.spawn("agreement", api.agreement_request(&self.app));
    }

    /// 登录后可后台发出的部分。
    fn post_login(&mut self, api: &Api) {
        self.spawn("getPromotionInfo", api.promotion_request(&self.app));
        self.spawn("getLoginUserInfo", api.login_user_info_request(&self.app));
        self.spawn("getSystemConfig", api.system_config_request(&self.app));
    }

    /// 失败只写 debug 日志；句柄登记以便退出前等待。
    fn spawn(&mut self, tag: &'static str, req: Request) {
        self.pending.push(std::thread::spawn(move || {
            if let Err(e) = req.send() {
                log::debug(&format!("附属请求[{tag}]失败：{}", e.log_text()));
            }
        }));
    }

    /// 轮询间隔（带抖动）。
    fn poll_delay_ms(&self) -> u64 {
        let lo = self.policy.poll_min_ms;
        let hi = self.policy.poll_max_ms;
        if hi <= lo {
            return lo;
        }
        lo + (proto::enc::random_u64() % (hi - lo + 1))
    }
}
