//! `sdo-ffxiv-launcher` — FFXIV CN（盛趣）自研启动器。
//!
//! 流程：设备档案 → 登录前附属请求 → 登录链（QR/push/fast）
//! → SSO 换票 → 登录后附属请求 → 写 SSO Cookie
//! → 构造启动参数 → 设置交接环境变量 → 启动游戏。

mod areas;
mod auxreq;
mod cli;
mod cookie;
mod ctx;
mod error;
mod game;
mod http;
mod login;
mod qr;
mod qrimage;
mod qrwin32;
mod qrwindow;
mod selfcheck;
mod single;
mod sso;
mod ui;
mod winproc;
mod winstr;

use clap::Parser;

use proto::consts::*;
use proto::device::Device;
use proto::enc;
use proto::log;
use proto::paths;

use cli::Args;
use ctx::{Ctx, GameTicket};
use error::{Error, Result};

fn main() {
    let args = Args::parse();
    let log_path = log::init(LOG_LAUNCHER, true, args.log_file.as_deref());
    ui::enable_vt();
    let code = match run(&args, &log_path) {
        Ok(()) => 0,
        Err(Error::UserQuit) => {
            println!("\n已按 q 退出。");
            0
        }
        Err(Error::Msg(m)) => {
            log::error(&format!("错误：{}", log::sanitize(&m)));
            1
        }
    };
    log::flush();
    std::process::exit(code);
}

fn run(args: &Args, log_path: &std::path::Path) -> Result<()> {
    args.validate()?;

    println!("sdo-ffxiv-launcher {}", env!("CARGO_PKG_VERSION"));
    println!("日志：{}", log_path.display());

    let _guard = match single::acquire() {
        single::Acquire::Guard(g) => g,
        single::Acquire::AlreadyRunning => {
            return Err(Error::msg("已有实例在运行，本次退出"))
        }
        single::Acquire::Failed(code) => {
            return Err(Error::msg(format!("启动失败，请重试（错误码 {code}）")))
        }
    };

    let (mut device, created) = Device::load_or_create()?;
    if created {
        println!("已生成新的设备档案 device.json");
    }
    let overridden = args.mac_id.is_some() || args.ep_name.is_some() || args.ep_ip.is_some();
    if let Some(v) = &args.mac_id {
        device.mac_id = v.clone();
    }
    if let Some(v) = &args.ep_name {
        device.ep_name = v.clone();
    }
    if let Some(v) = &args.ep_ip {
        device.ep_ip = v.clone();
    }
    if overridden {
        device.repair()?;
        let path = paths::cwd_file(FILE_DEVICE);
        device
            .save_atomic(&path)
            .map_err(|e| Error::msg(format!("写 device.json 失败: {e}")))?;
        println!("已应用命令行设备覆盖并保存");
    }
    log::info(&format!("设备就绪，本机 IP {}", device.ep_ip));

    let run_time_id = enc::new_run_time_id();
    let game = game::resolve(args.game_dir.as_deref());

    if args.self_check {
        return selfcheck::run(args, &device, &run_time_id, game.ok()).map_err(Error::msg);
    }

    let game = game.map_err(Error::msg)?;
    println!("游戏目录：{}", game.game_dir.display());

    game::verify_login_dll(&game.game_dir, args.skip_dll_check).map_err(Error::msg)?;

    let table = areas::fetch_table()?;
    let pick =
        areas::resolve_area(&table, args.area.as_deref(), device.last_area_id.as_deref())?;
    let (area, from_last) = (pick.area, pick.from_last);
    let base = proto::server::build_base(&area).map_err(Error::msg)?;
    if from_last {
        println!("使用上次的大区：{}；用 --area <id> 可临时更换", area.name);
    }
    println!("已选定大区：{}", area.name);
    log::info(&format!("已选定大区 {}", area.name));

    let mut ctx = Ctx {
        device: device.clone(),
        device_path: paths::cwd_file(FILE_DEVICE),
        run_time_id,
        args: args.clone(),
    };

    // 附属请求先发出，失败不阻断。
    auxreq::pre_login();

    let keep_flag = args.keep_login_flag();
    let mut round = 0u32;
    let (login_ticket, game_ticket) = loop {
        round += 1;
        if round > MAX_LOGIN_ROUNDS {
            return Err(Error::msg(format!(
                "换票连续 {MAX_LOGIN_ROUNDS} 轮失败，已终止"
            )));
        }
        let login_ticket = login::login(&mut ctx, keep_flag)?;
        match sso::exchange(&ctx, &login_ticket, &area.id) {
            Ok(game_ticket) => break (login_ticket, game_ticket),
            Err(e) => {
                let reason = log::sanitize(&e.to_string());
                log::info(&format!("换票失败：{reason}"));
                println!(
                    "换票失败（{reason}），重新扫码登录（第 {round}/{MAX_LOGIN_ROUNDS} 轮）"
                );
            }
        }
    };

    auxreq::post_login(&ctx, &login_ticket, &area.id)?;

    cookie::write_sso_cookies(&game_ticket.ticket);

    std::env::set_var(ENV_TICKET, &game_ticket.ticket);
    std::env::set_var(ENV_SNDAID, &game_ticket.snda_id);
    std::env::set_var(ENV_AREAID, &area.id);
    std::env::set_var(ENV_BASE, &base);

    assert_delivery(&game_ticket, &area.id, &base)?;

    let child = winproc::launch(&game.exe, &game.game_dir, &base).map_err(Error::msg)?;
    println!("游戏已启动。");

    // 启动成功才记住大区。
    if device.last_area_id.as_deref() != Some(area.id.as_str()) {
        match device.set_last_area_id(&area.id, &ctx.device_path) {
            Ok(()) => println!("已记住大区 {}，下次默认使用", area.name),
            Err(e) => log::info(&format!("写 lastAreaId 失败（不影响启动）：{e}")),
        }
    }

    if args.stay {
        println!("等待游戏退出…");
        child.wait();
        cookie::clear_sso_cookies();
        println!("游戏已退出，登录信息已清理。");
    }
    Ok(())
}

/// 校验刚写入环境的票据与大区参数和内存值一致，不一致返回用户可读错误。
fn assert_delivery(deliver: &GameTicket, area_id: &str, base: &str) -> Result<()> {
    let env_ticket = std::env::var(ENV_TICKET).unwrap_or_default();
    let env_snda = std::env::var(ENV_SNDAID).unwrap_or_default();
    let env_area = std::env::var(ENV_AREAID).unwrap_or_default();
    let env_base = std::env::var(ENV_BASE).unwrap_or_default();

    if env_ticket.is_empty() || env_snda.is_empty() {
        return Err(Error::msg("票据异常：请重试"));
    }
    if env_ticket != deliver.ticket || env_snda != deliver.snda_id {
        return Err(Error::msg("票据异常：请重试"));
    }
    if env_area != area_id || !base.contains(&format!("-AreaID={area_id} ")) {
        return Err(Error::msg("大区参数不一致，请重试"));
    }
    if env_base != base {
        return Err(Error::msg("启动参数不一致，请重试"));
    }
    Ok(())
}
