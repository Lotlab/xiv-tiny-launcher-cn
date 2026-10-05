//! `sdo-ffxiv-launcher` — FFXIV CN（盛趣）自研启动器。
//!
//! 流程：设备档案 → 登录前附属请求 → 登录链（QR/push/fast）
//! → SSO 换票 → 登录后附属请求
//! → 构造启动参数 → 设置交接环境变量 → 启动游戏。

mod areas;
mod cli;
mod ctx;
mod error;
mod game;
mod login;
mod qr;
mod qrimage;
/// 原生二维码窗口只在 Windows 上有实现（见 `qrwindow`）。
#[cfg(windows)]
mod qrwin32;
mod qrwindow;
mod selfcheck;
mod single;
mod ui;
mod winproc;

use clap::Parser;

use proto::consts::*;
use proto::device::Device;
use proto::enc;
use proto::log;
use proto::paths;

use cli::Args;
use ctx::Ctx;
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

/// 退出前等一等后台附属请求：它们是分离的线程，`process::exit` 不会等。
///
/// 用 `Drop` 而不是在 `run` 末尾显式调用，是为了让 `?` 提前返回的路径也覆盖到。
struct AuxWait<'a>(&'a sdo_client::Client);

impl Drop for AuxWait<'_> {
    fn drop(&mut self) {
        self.0
            .wait_pending(std::time::Duration::from_millis(AUX_WAIT_BUDGET_MS));
    }
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
        single::Acquire::Failed(e) => {
            let path = paths::lock_file();
            return Err(Error::msg(format!(
                "无法获取单实例锁（{}）：{e}",
                path.display()
            )));
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

    // 尽早解析启动方式：缺兼容层时不必等扫码完再失败。
    let launcher = winproc::resolve_launcher(args.run_via.as_deref()).map_err(Error::msg)?;
    if let Some(l) = &launcher {
        println!("通过兼容层启动游戏：{l}");
    }

    let net = sdo_client::Client::new(
        sdo_client::Identity::from(&device),
        run_time_id,
    );
    // 本会话的附属请求都挂在这个 `net` 上；退出时（含提前 `?` 返回）由它统一收尾。
    let _aux = AuxWait(&net);
    let table = areas::fetch_table(&net)?;
    let pick =
        areas::resolve_area(&table, args.area.as_deref(), device.last_area_id.as_deref())?;
    let (area, from_last) = (pick.area, pick.from_last);
    let base = sdo_client::server::build_base(&area)?;
    if from_last {
        println!("使用上次的大区：{}；用 --area <id> 可临时更换", area.name);
    }
    println!("已选定大区：{}", area.name);
    log::info(&format!("已选定大区 {}", area.name));

    let mut ctx = Ctx {
        device: device.clone(),
        device_path: paths::cwd_file(FILE_DEVICE),
        args: args.clone(),
    };

    // 附属请求先发出，失败不阻断。
    net.pre_login();

    let keep_flag = args.keep_login_flag();
    // 游戏应用口径：与登录应用同一种类型，按次传入选区。
    let game_app = sdo_client::App::game(area.id.clone());
    let mut round = 0u32;
    let (login_ticket, game_ticket) = loop {
        round += 1;
        if round > MAX_LOGIN_ROUNDS {
            return Err(Error::msg(format!(
                "换票连续 {MAX_LOGIN_ROUNDS} 轮失败，已终止"
            )));
        }
        let login_ticket = login::login(&mut ctx, keep_flag, &net)?;
        match net.exchange(&login_ticket, &game_app) {
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

    net.post_login_fire_and_forget(&login_ticket.tgt);
    net.check_face_verify(&login_ticket.tgt)?;

    // 票据与大区参数直接交给子进程的环境块（游戏侧 DLL 只读），不再改本进程环境。
    let delivery = winproc::Delivery {
        ticket: &game_ticket.ticket,
        snda_id: &game_ticket.snda_id,
        area_id: &area.id,
        base: &base,
    };

    let mut child = winproc::launch(&game.exe, &game.game_dir, &delivery, launcher.as_ref())
        .map_err(Error::msg)?;
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
        let _ = child.wait();
        println!("游戏已退出。");
    }
    Ok(())
}
