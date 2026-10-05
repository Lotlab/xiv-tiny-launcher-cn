//! 登录链的网络原语：单次端点执行 + 响应判定，无状态。
//!
//! 轮询循环、按键、`device.json` 落盘等编排在 EXE 侧 `launcher::login`。

use proto::consts::*;

use crate::client::{Client, Result};
use crate::endpoint::{
    CancelPush, CodeKeyLogin, FastInLogin, GetCodeKey, GetGuid, PushLogin, SendPush, Suffix,
};
use crate::error::Error;
use crate::resp;
use crate::tickets::LoginTicket;

/// 登录票据载荷（`codeKeyLogin` 与 `pushMessageLogin` 的成功结果形状相同）。
pub struct Ticket {
    pub ticket: String,
    pub tgt: String,
    pub keep_login_key: Option<String>,
}

/// 轮询单次结果（`codeKeyLogin` / `pushMessageLogin` 共用）。
pub enum Poll {
    /// `200 && return_code == 0 && ticket/sndaId/tgt 非空`。
    Success(Ticket),
    /// 服务端表示“还没好”（未扫码 `-10515805` / 未确认 `-10516808`）：继续等。
    Pending,
    /// 其它错误码：停轮询（内容为展示用原文）。换码还是终止由调用方决定。
    Fatal(String),
}

/// 从成功响应里取票据载荷。
fn ticket_from(json: &serde_json::Value) -> Ticket {
    Ticket {
        ticket: resp::data_str(json, "ticket").unwrap_or_default(),
        tgt: resp::data_str(json, "tgt").unwrap_or_default(),
        keep_login_key: resp::data_str(json, "keepLoginKey").filter(|k| !k.is_empty()),
    }
}

/// `fastInLogin` 成功结果（`guid` 为 `null` 时已现调一次 `getGuid` 补齐）。
pub struct FastSuccess {
    pub ticket: LoginTicket,
    pub new_keep_key: Option<String>,
}

/// 供自检调用二维码接口（不扫码）。
pub struct QrProbe {
    pub guid: String,
    pub bytes: usize,
    pub has_codekey_png: bool,
}

impl Client {
    fn login_suffix(&self) -> Suffix {
        Suffix::login(self.identity(), self.run_time_id(), self.app())
    }

    /// `getGuid`：`HTTP != 200`/无 `guid` 直接报错（调用方不再重试，直接退出）。
    pub fn get_guid(&self) -> Result<String> {
        let r = self.get(&GetGuid::new(self.login_suffix()))?;
        if r.status != 200 {
            return Err(Error::http(
                r.status,
                format!("登录服务繁忙（HTTP {}），请重试", r.status),
            ));
        }
        let json = r.json()?;
        match resp::data_str(&json, "guid").filter(|g| !g.is_empty()) {
            Some(g) => Ok(g),
            None => Err(Error::parse("登录服务返回异常，请重试")),
        }
    }

    /// 取二维码：响应体必须是 PNG（魔数 `89 50 4E 47`），`codeKey` 取自响应头 `CODEKEY`。
    /// 返回 `(png 字节, code_key)`。
    pub fn get_code_key(&self) -> Result<(Vec<u8>, String)> {
        let r = self.get(&GetCodeKey::new(self.login_suffix()))?;
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
        Ok((r.body, code_key))
    }

    /// 单次轮询。`Err` = 传输失败/`HTTP != 200`/解析失败（调用方计入 `attempt++` 继续等）。
    pub fn poll_code_key_once(
        &self,
        code_key: &str,
        guid: &str,
        keep_flag: i32,
    ) -> Result<Poll> {
        let ep = CodeKeyLogin::new(self.login_suffix(), code_key, guid, keep_flag);
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(r.status, "扫码轮询失败，请重试")
                .with_detail(format!("扫码轮询状态码 {}", r.status)));
        }
        let json = r.json()?;
        if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            return Ok(Poll::Success(ticket_from(&json)));
        }
        if resp::return_code(&json) == Some(RC_QR_NOT_SCANNED) {
            return Ok(Poll::Pending);
        }
        Ok(Poll::Fatal(resp::fail_reason_text(&json)))
    }

    /// `fastInLogin`（auto 模式优先）。`Err` = 回退 QR 的原因；
    /// 失败时调用方删本地 key，**不重发**，回退 QR。
    pub fn fast_login(&self, key: &str) -> Result<FastSuccess> {
        let suffix = Suffix::login_no_group(self.identity(), self.run_time_id(), self.app());
        let json = match self.get(&FastInLogin::new(suffix, key)) {
            Ok(r) if r.status == 200 => match r.json() {
                Ok(j) => j,
                Err(e) => return Err(Error::parse(format!("自动登录解析失败: {e}"))),
            },
            Ok(r) => return Err(Error::http(r.status, "自动登录服务繁忙，请重试")),
            Err(e) => return Err(Error::transport(format!("自动登录请求失败: {e}"))),
        };
        if !resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            return Err(Error::rejected(format!(
                "自动登录被拒绝：{}",
                resp::fail_reason_text(&json)
            )));
        }
        let ticket = resp::data_str(&json, "ticket").unwrap_or_default();
        let tgt = resp::data_str(&json, "tgt").unwrap_or_default();
        let new_keep_key = resp::data_str(&json, "keepLoginKey").filter(|k| !k.is_empty());
        let guid_resp = resp::data_str(&json, "guid").filter(|g| !g.is_empty() && g != "null");
        let guid = match guid_resp {
            Some(g) => g,
            None => match self.get_guid() {
                Ok(g) => g,
                Err(e) => return Err(Error::transport(format!("自动登录后取 guid 失败: {e}"))),
            },
        };
        Ok(FastSuccess {
            ticket: LoginTicket { ticket, tgt, guid },
            new_keep_key,
        })
    }

    /// 空 key 的 `cancelPushMessageLogin` 必回 `-10242301`，忽略结果。
    pub fn cancel_push(&self, guid: &str) {
        let _ = self.get(&CancelPush::new(self.login_suffix(), guid));
    }

    /// `sendPushMessage` 单次发送。`Ok` = 拿到 `pushMsgSessionKey`；
    /// `Err` = 可重试原因（调用方等 1s 后从 cancel 起完整重试，最多 3 次，之后转 QR）。
    pub fn send_push_once(&self, account: &str, guid: &str) -> Result<String> {
        let ep = SendPush::new(self.login_suffix(), account, guid);
        let send_json = match self.get(&ep) {
            Ok(r) if r.status == 200 => match r.json() {
                Ok(j) => j,
                Err(e) => return Err(Error::parse(format!("手机推送解析失败: {e}"))),
            },
            Ok(r) => return Err(Error::http(r.status, format!("手机推送 HTTP {}", r.status))),
            Err(e) => return Err(Error::transport(format!("手机推送请求失败: {e}"))),
        };
        if resp::return_code(&send_json) != Some(RC_OK) {
            return Err(Error::rejected(resp::fail_reason_text(&send_json)));
        }
        let session_key = resp::data_str(&send_json, "pushMsgSessionKey").unwrap_or_default();
        if session_key.is_empty() {
            return Err(Error::rejected("手机推送未返回会话标识"));
        }
        Ok(session_key)
    }

    /// 单次 push 轮询。`Err` = 传输失败/`HTTP != 200`/解析失败（调用方计入 `attempt++` 继续等）。
    pub fn poll_push_once(&self, session_key: &str, guid: &str) -> Result<Poll> {
        let ep = PushLogin::new(self.login_suffix(), session_key, guid);
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(Error::http(r.status, "手机确认轮询失败，请重试")
                .with_detail(format!("手机确认轮询状态码 {}", r.status)));
        }
        let json = r.json()?;
        if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
            return Ok(Poll::Success(ticket_from(&json)));
        }
        if resp::return_code(&json) == Some(RC_PUSH_NOT_CONFIRMED) {
            return Ok(Poll::Pending);
        }
        Ok(Poll::Fatal(resp::fail_reason_text(&json)))
    }

    /// 二维码接口探测（不扫码，供自检调用）。
    pub fn probe_qr(&self) -> Result<QrProbe> {
        let guid = self.get_guid()?;
        let r = self.get(&GetCodeKey::new(self.login_suffix()))?;
        if r.status != 200 {
            return Err(Error::http(r.status, "二维码服务繁忙，请重试"));
        }
        let has_codekey = resp::extract_codekey(r.header_values("set-cookie")).is_some();
        Ok(QrProbe {
            guid,
            bytes: r.body.len(),
            has_codekey_png: has_codekey && resp::is_png(&r.body),
        })
    }
}
