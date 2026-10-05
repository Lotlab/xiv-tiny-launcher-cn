//! `sdo-ffxiv-launcher` — FFXIV CN（盛趣）自研启动器。
//!
//! 网络与流程都在 `sdo-client`（`Api` / `Flow`）；这里只做 UI、落盘、起进程。

mod areas;
mod cli;
mod cmdline;
mod consts;
mod error;
mod game;
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

use proto::consts::FILE_DEVICE;
use proto::device::Device;
use proto::enc;
use proto::log;
use proto::paths;

use cli::Args;
use consts::LOG_LAUNCHER;
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

    // 手机确认账号：`--account` 优先，否则用上次记住的。放在这里是为了在触网前就报错。
    let stored_account = device.last_account.clone();
    let account = args.account_or(stored_account.as_deref());
    // auto 的回退链来自上次成功的那条；`--mode qr`/`push` 时这个值不用。
    let last_chain = sdo_client::Chain::from_tag(
        device.last_login_method.as_deref().unwrap_or_default(),
        account.clone(),
    );
    let method = args.method(last_chain, stored_account.as_deref())?;

    game::verify_login_dll(&game.game_dir, args.skip_dll_check).map_err(Error::msg)?;

    // 尽早解析启动方式：缺兼容层时不必等扫码完再失败。
    let launcher = winproc::resolve_launcher(args.run_via.as_deref()).map_err(Error::msg)?;
    if let Some(l) = &launcher {
        println!("通过兼容层启动游戏：{l}");
    }

    let mut api = sdo_client::Api::new(sdo_client::Identity::from(&device), run_time_id);

    // 区服表：拉取在层 1，缓存回退与落盘在这里。
    let table = areas::fetch_table(&api)?;
    let pick = areas::resolve_area(&table, args.area.as_deref(), device.last_area_id.as_deref())?;
    let (area, from_last) = (pick.area, pick.from_last);
    let base = cmdline::Builder::new(&area).build()?;
    if from_last {
        println!("使用上次的大区：{}；用 --area <id> 可临时更换", area.name);
    }
    println!("已选定大区：{}", area.name);
    log::info(&format!("已选定大区 {}", area.name));

    let mut flow = sdo_client::Flow::new(
        args.policy(),
        method,
        sdo_client::LOGIN_APP,
        sdo_client::App::game(area.id),
    );
    let mut session_ui = ui::TerminalUi::new(
        args.qr_render,
        args.qr_out.clone(),
        args.keep_login(),
    );
    let outcome = flow.run(&mut api, &mut session_ui, device.keep_login_key.as_deref());

    // 无论成败：续登凭据的决定要落盘，后台附属请求要收尾。
    apply_keep_key(&mut device, flow.keep_key());
    flow.wait_pending();

    let game_ticket = match outcome {
        Ok(t) => t,
        Err(e) if e.is_quit() => return Err(Error::UserQuit),
        Err(e) => return Err(e.into()),
    };

    // 票据与大区参数直接交给子进程的环境块（游戏侧 DLL 只读），不再改本进程环境。
    let area_id_text = area.id.to_string();
    let delivery = winproc::Delivery {
        ticket: &game_ticket.ticket,
        snda_id: &game_ticket.snda_id,
        area_id: &area_id_text,
        base: &base,
    };

    let mut child = winproc::launch(&game.exe, &game.game_dir, &delivery, launcher.as_ref())
        .map_err(Error::msg)?;
    println!("游戏已启动。");

    // 启动成功才记住大区。
    if device.last_area_id.as_deref() != Some(area_id_text.as_str()) {
        match device.set_last_area_id(&area_id_text, &paths::cwd_file(FILE_DEVICE)) {
            Ok(()) => println!("已记住大区 {}，下次默认使用", area.name),
            Err(e) => log::info(&format!("写 lastAreaId 失败（不影响启动）：{e}")),
        }
    }

    // 同样只在启动成功后才记：下次 auto 的续登凭据失败时回退到这条链、用这个账号。
    if let Some(chain) = flow.chain_used() {
        let path = paths::cwd_file(FILE_DEVICE);
        if device.last_login_method.as_deref() != Some(chain.tag()) {
            if let Err(e) = device.set_last_login_method(chain.tag(), &path) {
                log::info(&format!("写 lastLoginMethod 失败（不影响启动）：{e}"));
            }
        }
        if let Some(a) = account.as_deref().filter(|a| !a.trim().is_empty()) {
            if device.last_account.as_deref() != Some(a) {
                if let Err(e) = device.set_last_account(a, &path) {
                    log::info(&format!("写 lastAccount 失败（不影响启动）：{e}"));
                }
            }
        }
    }

    if args.stay {
        println!("等待游戏退出…");
        let _ = child.wait();
        println!("游戏已退出。");
    }
    Ok(())
}

/// 落盘层 2 给出的续登凭据决定；失败只记日志（不该因为写档案失败而放弃已拿到的票据）。
fn apply_keep_key(device: &mut Device, decision: &sdo_client::KeepKey) {
    let value = match decision {
        sdo_client::KeepKey::Keep => return,
        sdo_client::KeepKey::Replace(k) => Some(k.clone()),
        sdo_client::KeepKey::Clear => None,
    };
    let path = paths::cwd_file(FILE_DEVICE);
    if let Err(e) = device.set_keep_login_key(value, &path) {
        log::debug(&format!("续登凭据落盘失败：{e}"));
    }
}
