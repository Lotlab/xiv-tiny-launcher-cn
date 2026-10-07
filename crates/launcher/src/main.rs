//! `sdo-ffxiv-launcher` — FFXIV CN（盛趣）自研启动器。
//!
//! 网络与流程都在 `sdo-client`（`Api` / `Flow`）；这里只做 UI、落盘、起进程。

mod areas;
mod cli;
mod cmdline;
mod consts;
mod dllswap;
mod error;
mod game;
mod proc;
mod qr;
mod qrimage;
/// 原生二维码窗口只在 Windows 上有实现（见 `qrwindow`）。
#[cfg(windows)]
mod qrwin32;
mod qrwindow;
mod selfcheck;
mod single;
mod ui;
mod update;
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
    // swap 握手 nonce：复用本进程的 run_time_id（每进程唯一，旧 marker 永不误命中）。
    let swap_nonce: Option<String> = args.swap_dll().then(|| run_time_id.clone());

    // 安装根（不要求 `game/` 存在——全新安装时还没有）。
    let root = game::resolve_root(args.game_dir.as_deref());
    // 已安装时解析游戏目录；全新安装要等更新阶段建出 `game/`。
    let game = game::resolve(args.game_dir.as_deref()).ok();

    if args.self_check {
        return selfcheck::run(args, &device, &run_time_id, game).map_err(Error::msg);
    }

    // 先做便宜的前置检查：目录已存在就立刻验登录组件，免得下完几十 GB
    // 才发现 DLL 缺失。`--check-update` 只查版本，不碰这些。
    // 瞬时替换默认开，自己管理文件（拿到票据后才换），这里跳过。
    let mut dll_checked = false;
    if !args.check_update && !args.swap_dll() {
        if let Some(g) = &game {
            game::verify_login_dll(&g.game_dir, args.skip_dll_check).map_err(Error::msg)?;
            dll_checked = true;
        }
    }

    // 更新阶段（全新安装会在这里建出 `game/`）。
    update::run(args, &root)?;
    if args.check_update {
        return Ok(());
    }

    // 更新可能刚建出 `game/`：全新安装要重新解析一次。
    let game = match game {
        Some(g) => g,
        None => game::resolve(args.game_dir.as_deref()).map_err(Error::msg)?,
    };
    if !dll_checked && !args.swap_dll() {
        game::verify_login_dll(&game.game_dir, args.skip_dll_check).map_err(Error::msg)?;
    }
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
    // 瞬时替换（默认开）：拿到票据后、起进程前才换上自研 DLL，窗口最小。
    let mut swap = None;
    if args.swap_dll() {
        let ours_src = dllswap::ours_source(args.ours_dll.as_deref());
        println!("瞬时替换：正在换上自研登录组件…");
        swap = Some(dllswap::prepare(&game.game_dir, &ours_src).map_err(Error::msg)?);
    }
    let delivery = winproc::Delivery {
        ticket: &game_ticket.ticket,
        snda_id: &game_ticket.snda_id,
        area_id: &area_id_text,
        base: &base,
        swap_nonce: swap_nonce.as_deref(),
    };

    let mut child = winproc::launch(&game.exe, &game.game_dir, &delivery, launcher.as_ref())
        .map_err(Error::msg)?;
    // `spawn` 成功只代表进程建出来了：缺运行库 / DLL 被拦截 / wine 报错时游戏可能
    // 几百毫秒内就退出。先确认熬过启动期，活着才报“已启动”，秒退直接报错。
    let launch_at = std::time::Instant::now();
    let pid = child.id();
    println!("已发出启动命令（pid {pid}），正在确认游戏进程…");
    if let Err(exit) = winproc::confirm_running(&mut child, winproc::HEALTH_GRACE) {
        let code_hint = match exit.code {
            Some(c) => format!("退出码 {c}"),
            None => exit.status.clone(),
        };
        return Err(Error::msg(format!(
            "游戏进程已在 {:?} 内退出（pid {}，{code_hint}），没有真正跑起来。\n\
             请检查：游戏路径与权限、DirectX/VC++ 运行库、登录组件是否被杀软拦截、兼容层（wine）报错；\n\
             用 --stay 重跑可保留控制台查看完整过程，详情见日志。",
            exit.after, exit.pid,
        )));
    }
    println!("游戏已启动（pid {pid}）。");
    if !args.stay {
        println!("启动器即将退出，游戏继续运行；若稍后发现游戏没窗口，请用 --stay 重跑以便查看报错。");
    }

    // 瞬时替换：等游戏加载完就把官方换回去（fail-safe：失败留自研在位）。
    if let Some(sw) = swap.as_mut() {
        let pid = child.id();
        let nonce = swap_nonce.as_deref().unwrap_or("");
        println!("等待游戏加载登录组件（最多 {:?}）…", dllswap::WAIT_TIMEOUT);
        let alive = || child.try_wait().map(|e| e.is_none()).unwrap_or(true);
        match dllswap::wait_game_loaded(pid, &sw.sdo_dir, nonce, alive) {
            Ok(()) => match dllswap::restore(sw) {
                Ok(()) => println!("已换回官方登录组件，游戏继续使用已加载的自研组件。"),
                Err(e) => log::warn(&format!("换回官方 DLL 失败（盘面留自研，不影响本次游戏）：{e}")),
            },
            Err(e) => {
                // 等待期间游戏自己退了：这不是“换回失败”，是启动失败，必须报错
                // （之前只 warn，会误报“游戏已启动”）。游戏还活着才按超时 warn。
                let dead = child
                    .try_wait()
                    .map(|o| o.is_some())
                    .unwrap_or(false)
                    && !proc::is_running(consts::GAME_EXE);
                if dead {
                    let status = child
                        .try_wait()
                        .ok()
                        .flatten()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| "未知状态".to_string());
                    return Err(Error::msg(format!(
                        "等待登录组件期间游戏已退出（{status}）：{e}\n\
                         请检查登录组件与运行库，详情见日志。"
                    )));
                }
                log::warn(&e);
            }
        }
    }

    // 启动成功：更新确实完成且游戏能跑起来，可以清掉临时目录。
    update::cleanup_work(&root);

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
        let status = child.wait().map(|s| s.to_string()).unwrap_or_else(|e| format!("等待失败：{e}"));
        println!("游戏已退出（{status}）。");
        if launch_at.elapsed() < std::time::Duration::from_secs(15) {
            println!("提示：游戏在 {:?} 内就退出了，大概率没正常进大厅，请按上面的报错排查运行库/登录组件/兼容层。", launch_at.elapsed());
        }
        // 瞬时替换：游戏已退，藏匿文件解锁，删掉它。
        if let Some(sw) = swap.as_ref() {
            dllswap::cleanup(sw);
        }
    } else if swap.is_some() {
        log::info("瞬时替换的藏匿文件将在下次启动时自动清理");
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
