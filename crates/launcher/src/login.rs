//! 登录交互编排：模式分发、二维码/手机确认轮询循环、按键与渲染。
//! 单次网络原语在 `sdo_client::login`，本模块只做循环、倒计时、按键与落盘。

use std::time::{Duration, Instant};

use proto::consts::*;
use proto::log;

use crate::ctx::{Ctx, LoginTicket};
use crate::error::{Error, Result};
use crate::qr;
use crate::ui::{self, Key};

use sdo_client::Client;
use sdo_client::login::{CodeKeyPoll, PushPoll};

pub use sdo_client::login::QrProbe;

/// 登录链结果。`Fallback` 里是给用户的回退原因。
pub enum Chain {
    Ok(LoginTicket),
    Fallback(String),
    Quit,
}

/// 按 `--mode` 选择登录链。
pub fn login(ctx: &mut Ctx, keep_flag: i32) -> Result<LoginTicket> {
    let net = Client::new(
        sdo_client::Identity::from(&ctx.device),
        ctx.run_time_id.clone(),
    );
    match ctx.args.mode {
        crate::cli::Mode::Push => {
            let account = ctx.args.account.clone().unwrap_or_default();
            match push_login(ctx, &account, &net) {
                Chain::Ok(t0) => Ok(t0),
                Chain::Quit => Err(Error::UserQuit),
                Chain::Fallback(reason) => {
                    log::warn(&format!(
                        "手机登录不可用（{}），转为二维码登录",
                        log::sanitize(&reason)
                    ));
                    qr_login(ctx, keep_flag, &net)
                }
            }
        }
        crate::cli::Mode::Qr => qr_login(ctx, keep_flag, &net),
        crate::cli::Mode::Auto => {
            let Some(key) = ctx.device.keep_login_key.clone() else {
                return qr_login(ctx, keep_flag, &net);
            };
            match net.fast_login(&key) {
                Ok(ok) => {
                    if let Some(new_key) = ok.new_keep_key {
                        let path = ctx.device_path.clone();
                        if let Err(e) = ctx
                            .device
                            .set_keep_login_key(Some(new_key), &path)
                        {
                            log::debug(&format!("登录信息更新细节：{e}"));
                        }
                    }
                    Ok(ok.ticket)
                }
                Err(reason) => {
                    log::warn(&format!(
                        "自动登录失败（{}），转为二维码登录",
                        log::sanitize(&reason)
                    ));
                    let path = ctx.device_path.clone();
                    if let Err(e) = ctx.device.set_keep_login_key(None, &path) {
                        log::debug(&format!("清除登录信息写盘失败：{e}"));
                    }
                    qr_login(ctx, keep_flag, &net)
                }
            }
        }
    }
}

/// 二维码主流程：出码→轮询→换码循环。
pub fn qr_login(ctx: &mut Ctx, keep_flag: i32, net: &Client) -> Result<LoginTicket> {
    let guid = net.get_guid().map_err(Error::msg)?;
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
        let (png, code_key) = net.get_code_key().map_err(Error::msg)?;
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
                    log::debug(&format!(
                        "二维码窗口不可用：{reason}（{}）",
                        crate::qrwindow::environment_facts()
                    ));
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

            match net.poll_code_key_once(&code_key, &guid, keep) {
                Ok(CodeKeyPoll::Success {
                    ticket,
                    tgt,
                    keep_login_key,
                }) => {
                    let login_ticket =
                        finish_login(ctx, ticket, tgt, &guid, keep_login_key)?;
                    ui::status("扫码成功");
                    ui::status_end();
                    return Ok(login_ticket);
                }
                Ok(CodeKeyPoll::NotScanned) => {
                    attempt += 1;
                    ui::status(&format!("[{left:>3}s] 等待扫码"));
                }
                Ok(CodeKeyPoll::ServerError(text)) => {
                    log::debug(&format!("扫码返回错误：{text}"));
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

/// 成功响应组装票据；有保持登录信息就存盘。
fn finish_login(
    ctx: &mut Ctx,
    ticket: String,
    tgt: String,
    guid: &str,
    keep_login_key: Option<String>,
) -> Result<LoginTicket> {
    if let Some(k) = &keep_login_key {
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

/// 手机确认登录：失败按常量重试，耗尽后转二维码。
pub fn push_login(ctx: &mut Ctx, account: &str, net: &Client) -> Chain {
    if account.trim().is_empty() {
        return Chain::Fallback("未提供 --account".into());
    }
    for round in 1..=PUSH_SEND_MAX_TRIES {
        let guid = match net.get_guid() {
            Ok(g) => g,
            Err(e) => return Chain::Fallback(format!("手机登录取 guid 失败: {e}")),
        };
        net.cancel_push(&guid);
        let session_key = match net.send_push_once(account, &guid) {
            Ok(k) => k,
            Err(reason) => {
                println!(
                    "手机推送发送失败，正在重试（第 {round}/{PUSH_SEND_MAX_TRIES} 次）：{reason}"
                );
                std::thread::sleep(Duration::from_millis(PUSH_SEND_RETRY_WAIT_MS));
                continue;
            }
        };
        println!("已发送手机确认请求（请在手机 App 上确认），等待确认…");
        match poll_push(ctx, &session_key, &guid, net) {
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

fn poll_push(ctx: &mut Ctx, session_key: &str, guid: &str, net: &Client) -> Result<LoginTicket> {
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
        match net.poll_push_once(session_key, guid)
        {
            Ok(PushPoll::Success {
                ticket,
                tgt,
                keep_login_key,
            }) => {
                return finish_login(ctx, ticket, tgt, guid, keep_login_key);
            }
            Ok(PushPoll::NotConfirmed) => {
                attempt += 1;
                let left = deadline.saturating_duration_since(Instant::now()).as_secs();
                ui::status(&format!("[{left:>3}s] 等待手机确认"));
            }
            Ok(PushPoll::Rejected(reason)) => {
                return Err(Error::msg(format!(
                    "手机确认登录被服务端拒绝：{reason}"
                )));
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
pub fn probe_qr(ctx: &Ctx) -> Result<QrProbe> {
    let net = Client::new(
        sdo_client::Identity::from(&ctx.device),
        ctx.run_time_id.clone(),
    );
    net.probe_qr().map_err(Error::msg)
}
