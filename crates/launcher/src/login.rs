//! 登录链：二维码、快速登录、手机确认登录。

use std::time::{Duration, Instant};

use proto::consts::*;
use proto::log;
use proto::params::{self, Suffix};
use proto::resp;

use crate::ctx::{Ctx, LoginTicket};
use crate::error::{Error, Result};
use crate::http::{self, Timeout};
use crate::qr;
use crate::ui::{self, Key};

/// 登录链结果。`Fallback` 里是给用户的回退原因。
pub enum Chain {
    Ok(LoginTicket),
    Fallback(String),
    Quit,
}

/// 按 `--mode` 选择登录链。
pub fn login(ctx: &mut Ctx, keep_flag: i32) -> Result<LoginTicket> {
    match ctx.args.mode {
        crate::cli::Mode::Push => {
            let account = ctx.args.account.clone().unwrap_or_default();
            match push_login(ctx, &account) {
                Chain::Ok(t0) => Ok(t0),
                Chain::Quit => Err(Error::UserQuit),
                Chain::Fallback(reason) => {
                    log::warn(&format!(
                        "手机登录不可用（{}），转为二维码登录",
                        log::sanitize(&reason)
                    ));
                    qr_login(ctx, keep_flag)
                }
            }
        }
        crate::cli::Mode::Qr => qr_login(ctx, keep_flag),
        crate::cli::Mode::Auto => {
            let Some(key) = ctx.device.keep_login_key.clone() else {
                return qr_login(ctx, keep_flag);
            };
            match fast_login(ctx, &key) {
                Chain::Ok(t0) => Ok(t0),
                Chain::Quit => Err(Error::UserQuit),
                Chain::Fallback(reason) => {
                    log::warn(&format!(
                        "自动登录失败（{}），转为二维码登录",
                        log::sanitize(&reason)
                    ));
                    let path = ctx.device_path.clone();
                    if let Err(e) = ctx.device.set_keep_login_key(None, &path) {
                        log::debug(&format!("清除登录信息写盘失败：{e}"));
                    }
                    qr_login(ctx, keep_flag)
                }
            }
        }
    }
}

pub fn get_guid(ctx: &Ctx) -> Result<String> {
    let suffix = Suffix::login(&ctx.device, &ctx.run_time_id);
    let path = params::path_get_guid(&suffix);
    let r = http::get(HOST_CAS, &path, Timeout::Auth)?;
    if r.status != 200 {
        return Err(Error::msg(format!("登录服务繁忙（HTTP {}），请重试", r.status)));
    }
    let json = r.json()?;
    match resp::data_str(&json, "guid").filter(|g| !g.is_empty()) {
        Some(g) => Ok(g),
        None => Err(Error::msg("登录服务返回异常，请重试")),
    }
}

/// 二维码主流程：出码→轮询→换码循环。
pub fn qr_login(ctx: &mut Ctx, keep_flag: i32) -> Result<LoginTicket> {
    let suffix = Suffix::login(&ctx.device, &ctx.run_time_id);
    let guid = get_guid(ctx)?;
    let mut keep = keep_flag;
    let keys = ui::Keys::new();
    let mut code_round = 0u32;
    let mut consecutive_error_refresh = 0u32;

    loop {
        code_round += 1;
        if code_round > MAX_QR_CODE_ROUNDS {
            return Err(Error::msg(format!(
                "连续更换 {MAX_QR_CODE_ROUNDS} 张二维码仍未登录成功，终止"
            )));
        }
        let (png, code_key) = get_code_key(&suffix)?;
        let png_path = qr::save_png_at(&png, ctx.args.qr_out.as_deref())
            .map_err(|e| Error::msg(format!("二维码图片保存失败: {e}")))?;
        println!(
            "\n第 {code_round} 张二维码（也已保存到 {}）：",
            png_path.display()
        );

        let render_mode = ctx.args.qr_render;
        let mut window = None;
        if render_mode != crate::cli::QrRender::Ascii {
            match crate::qrwindow::show(&png) {
                Ok(w) => window = Some(w),
                Err(reason) => {
                    log::debug(&format!("二维码窗口不可用：{reason}"));
                }
            }
        }
        if window.is_none() {
            let want_color = ctx.args.qr_render == crate::cli::QrRender::Auto && ui::color_ok();
            if let Err(e) = qr::render_terminal_with(&png, want_color) {
                log::debug(&format!("终端二维码渲染细节：{e}"));
                println!("终端显示失败，请直接扫 {}", png_path.display())
            }
        } else {
            println!("已弹出二维码窗口（本张码结束时自动关闭）。");
        }
        println!(
            "按键：k=勾选保持登录  Ctrl+C=换一张码  q=退出\n\
             保持登录：{}  倒计时 {}s\n",
            if keep == KEEP_LOGIN_FLAG_CHECKED {
                "已勾选"
            } else {
                "未勾选（按 k 勾选）"
            },
            ctx.args.qr_timeout
        );

        let code_started = Instant::now();
        let deadline = code_started + Duration::from_secs(ctx.args.qr_timeout);
        let mut attempt = 0u32;
        let refresh_reason: String;
        let mut server_error = false;
        loop {
            if let Some(k) = keys.poll() {
                match k {
                    Key::Quit => return Err(Error::UserQuit),
                    Key::CancelCode => {
                        println!("\n已取消本码，正在重新获取二维码…");
                        refresh_reason = "手动取消".to_string();
                        break;
                    }
                    Key::KeepLogin => {
                        if keep != KEEP_LOGIN_FLAG_CHECKED {
                            keep = KEEP_LOGIN_FLAG_CHECKED;
                        }
                    }
                }
            }
            let left = deadline.saturating_duration_since(Instant::now()).as_secs();
            if Instant::now() >= deadline {
                ui::status("倒计时结束，更换二维码");
                ui::status_end();
                refresh_reason = format!("倒计时 {}s 到", ctx.args.qr_timeout);
                break;
            }

            let path = params::path_code_key_login(&suffix, &code_key, &guid, keep);
            match http::get(HOST_CAS, &path, Timeout::Auth) {
                Ok(r) if r.status == 200 => match r.json() {
                    Ok(json) => {
                        if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
                            let login_ticket = build_login_ticket(ctx, &json, &guid)?;
                            ui::status("扫码成功");
                            ui::status_end();
                            return Ok(login_ticket);
                        }
                        let rc = resp::return_code(&json);
                        if rc == Some(RC_QR_NOT_SCANNED) {
                            attempt += 1;
                            ui::status(&format!("[{left:>3}s] 等待扫码"));
                        } else {
                            let text = resp::fail_reason_text(&json);
                            log::debug(&format!("扫码返回错误 rc={rc:?}：{text}"));
                            ui::status(&format!("服务端返回错误：{text}"));
                            ui::status_end();
                            refresh_reason = "服务端返回错误".to_string();
                            server_error = true;
                            consecutive_error_refresh += 1;
                            if consecutive_error_refresh >= 3 {
                                return Err(Error::msg(format!(
                                    "登录被拒绝（连续 {consecutive_error_refresh} 次）：{text}"
                                )));
                            }
                            break;
                        }
                    }
                    Err(e) => {
                        attempt += 1;
                        log::debug(&format!("扫码轮询解析细节：{e}"));
                        ui::status(&format!("[{left:>3}s] 等待扫码"));
                    }
                },
                Ok(r) => {
                    attempt += 1;
                    log::debug(&format!("扫码轮询状态码 {}", r.status));
                    ui::status(&format!("[{left:>3}s] 等待扫码"));
                }
                Err(e) => {
                    attempt += 1;
                    log::debug(&format!("扫码轮询请求细节：{e}"));
                    ui::status(&format!("[{left:>3}s] 等待扫码"));
                }
            }

            if attempt >= ctx.args.qr_max_attempts {
                ui::status(&format!(
                    "连续 {} 次未成功，更换二维码",
                    ctx.args.qr_max_attempts
                ));
                ui::status_end();
                refresh_reason = format!("连续 {} 次未成功", ctx.args.qr_max_attempts);
                break;
            }
            std::thread::sleep(Duration::from_millis(ctx.poll_delay_ms()));
        }

        if !server_error {
            consecutive_error_refresh = 0;
        }
        let floor = Duration::from_millis(ctx.args.poll_min_ms);
        if let Some(rest) = floor.checked_sub(code_started.elapsed()) {
            std::thread::sleep(rest);
        }
        log::debug(&format!(
            "第 {code_round} 张码结束（{refresh_reason}）"
        ));
    }
}

/// 取二维码：响应体必须是 PNG。
fn get_code_key(suffix: &Suffix) -> Result<(Vec<u8>, String)> {
    let path = params::path_get_code_key(suffix);
    let r = http::get(HOST_CAS, &path, Timeout::Download)?;
    if r.status != 200 {
        return Err(Error::msg(format!("二维码服务繁忙（HTTP {}），请重试", r.status)));
    }
    if !resp::is_png(&r.body) {
        return Err(Error::msg("二维码响应异常，请重试"));
    }
    let code_key = resp::extract_codekey(r.header_values("set-cookie"))
        .ok_or_else(|| Error::msg("二维码响应异常，请重试"))?;
    Ok((r.body, code_key))
}

/// 成功响应组装票据；有保持登录信息就存盘。
pub fn build_login_ticket(ctx: &mut Ctx, json: &serde_json::Value, guid: &str) -> Result<LoginTicket> {
    let ticket = resp::data_str(json, "ticket").unwrap_or_default();
    let tgt = resp::data_str(json, "tgt").unwrap_or_default();
    let key = resp::data_str(json, "keepLoginKey").filter(|k| !k.is_empty());
    if let Some(k) = &key {
        let path = ctx.device_path.clone();
        ctx.device
            .set_keep_login_key(Some(k.clone()), &path)
            .map_err(|e| Error::msg(format!("登录信息保存失败: {e}")))?;
    }
    Ok(LoginTicket {
        ticket,
        tgt,
        guid: guid.to_string(),
    })
}

pub fn fast_login(ctx: &mut Ctx, key: &str) -> Chain {
    let suffix = Suffix::login_no_group(&ctx.device, &ctx.run_time_id);
    let path = params::path_fast_in_login(&suffix, key);
    let json = match http::get(HOST_N_CAS, &path, Timeout::Auth) {
        Ok(r) if r.status == 200 => match r.json() {
            Ok(j) => j,
            Err(e) => return Chain::Fallback(format!("自动登录解析失败: {e}")),
        },
        Ok(r) => return Chain::Fallback(format!("自动登录 HTTP {}", r.status)),
        Err(e) => return Chain::Fallback(format!("自动登录请求失败: {e}")),
    };
    if !resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
        return Chain::Fallback(format!("自动登录被拒绝：{}", resp::fail_reason_text(&json)));
    }
    let ticket = resp::data_str(&json, "ticket").unwrap_or_default();
    let tgt = resp::data_str(&json, "tgt").unwrap_or_default();
    if let Some(new_key) = resp::data_str(&json, "keepLoginKey").filter(|k| !k.is_empty()) {
        let path = ctx.device_path.clone();
        if let Err(e) = ctx.device.set_keep_login_key(Some(new_key.clone()), &path) {
            log::debug(&format!("登录信息更新细节：{e}"));
        }
    }
    let guid_resp = resp::data_str(&json, "guid").filter(|g| !g.is_empty() && g != "null");
    let guid = match guid_resp {
        Some(g) => g,
        None => match get_guid(ctx) {
            Ok(g) => g,
            Err(e) => return Chain::Fallback(format!("自动登录后取 guid 失败: {e}")),
        },
    };
    Chain::Ok(LoginTicket { ticket, tgt, guid })
}

/// 手机确认登录：失败按常量重试，耗尽后转二维码。
pub fn push_login(ctx: &mut Ctx, account: &str) -> Chain {
    if account.trim().is_empty() {
        return Chain::Fallback("未提供 --account".into());
    }
    let suffix = Suffix::login(&ctx.device, &ctx.run_time_id);
    for round in 1..=PUSH_SEND_MAX_TRIES {
        let guid = match get_guid(ctx) {
            Ok(g) => g,
            Err(e) => return Chain::Fallback(format!("手机登录取 guid 失败: {e}")),
        };
        let cancel = params::path_cancel_push_message_login(&suffix, &guid);
        let _ = http::get(HOST_CAS, &cancel, Timeout::Auth);
        let send_path = params::path_send_push_message(&suffix, account, &guid);
        let send_json = match http::get(HOST_CAS, &send_path, Timeout::Auth) {
            Ok(r) if r.status == 200 => match r.json() {
                Ok(j) => j,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(PUSH_SEND_RETRY_WAIT_MS));
                    continue;
                }
            },
            _ => {
                std::thread::sleep(Duration::from_millis(PUSH_SEND_RETRY_WAIT_MS));
                continue;
            }
        };
        if resp::return_code(&send_json) != Some(RC_OK) {
            println!(
                "手机推送发送失败，正在重试（第 {round}/{PUSH_SEND_MAX_TRIES} 次）：{}",
                resp::fail_reason_text(&send_json)
            );
            std::thread::sleep(Duration::from_millis(PUSH_SEND_RETRY_WAIT_MS));
            continue;
        }
        let session_key = resp::data_str(&send_json, "pushMsgSessionKey").unwrap_or_default();
        if session_key.is_empty() {
            std::thread::sleep(Duration::from_millis(PUSH_SEND_RETRY_WAIT_MS));
            continue;
        }
        println!("已发送手机确认请求（请在手机 App 上确认），等待确认…");
        match poll_push(ctx, &suffix, &session_key, &guid) {
            Ok(login_ticket) => return Chain::Ok(login_ticket),
            Err(Error::UserQuit) => {
                return Chain::Quit;
            }
            Err(e) => return Chain::Fallback(format!("手机确认结束: {e}")),
        }
    }
    Chain::Fallback(format!(
        "手机推送连续 {PUSH_SEND_MAX_TRIES} 次发送失败"
    ))
}

fn poll_push(ctx: &mut Ctx, suffix: &Suffix, session_key: &str, guid: &str) -> Result<LoginTicket> {
    let keys = ui::Keys::new();
    let deadline = Instant::now() + Duration::from_secs(ctx.args.qr_timeout);
    let mut attempt = 0u32;
    loop {
        if let Some(k) = keys.poll() {
            match k {
                Key::Quit => return Err(Error::UserQuit),
                Key::CancelCode => {
                    return Err(Error::msg("已取消手机确认"));
                }
                Key::KeepLogin => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::msg(format!(
                "手机确认 {}s 倒计时结束",
                ctx.args.qr_timeout
            )));
        }
        let path = params::path_push_message_login(suffix, session_key, guid);
        match http::get(HOST_CAS, &path, Timeout::Auth) {
            Ok(r) if r.status == 200 => match r.json() {
                Ok(json) => {
                    if resp::is_success(&json, &["ticket", "sndaId", "tgt"]) {
                        return build_login_ticket(ctx, &json, guid);
                    }
                    if resp::return_code(&json) == Some(RC_PUSH_NOT_CONFIRMED) {
                        attempt += 1;
                        let left = deadline.saturating_duration_since(Instant::now()).as_secs();
                        ui::status(&format!("[{left:>3}s] 等待手机确认"));
                    } else {
                        return Err(Error::msg(format!(
                            "手机确认登录被服务端拒绝：{}",
                            resp::fail_reason_text(&json)
                        )));
                    }
                }
                Err(_) => {
                    attempt += 1;
                }
            },
            Ok(_) => {
                attempt += 1;
            }
            Err(_) => {
                attempt += 1;
            }
        }
        if attempt >= ctx.args.qr_max_attempts {
            return Err(Error::msg(format!(
                "连续 {} 次未确认",
                ctx.args.qr_max_attempts
            )));
        }
        std::thread::sleep(Duration::from_millis(ctx.poll_delay_ms()));
    }
}

/// 供自检调用二维码接口（不扫码）。
pub struct QrProbe {
    pub guid: String,
    pub bytes: usize,
    pub has_codekey_png: bool,
}

pub fn probe_qr(ctx: &Ctx) -> Result<QrProbe> {
    let guid = get_guid(ctx)?;
    let suffix = Suffix::login(&ctx.device, &ctx.run_time_id);
    let path = params::path_get_code_key(&suffix);
    let r = http::get(HOST_CAS, &path, Timeout::Download)?;
    let has_codekey = resp::extract_codekey(r.header_values("set-cookie")).is_some();
    Ok(QrProbe {
        guid,
        bytes: r.body.len(),
        has_codekey_png: has_codekey && resp::is_png(&r.body),
    })
}
