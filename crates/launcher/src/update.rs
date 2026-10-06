//! 更新阶段：版本检查 / 增量更新 / 全量安装修复 + 终端进度。
//!
//! 默认（`UpdateMode::Auto`）：检查 → 已最新就跳过 → 预演增量链 → 有更新且
//! 本地有 internal 版本就走增量，否则全量。真正下载前会问一次「是否现在更新」
//! （`--yes` 可跳过；非交互式运行问不了时**跳过更新继续登录**，只有
//! `--force-full` 才强制要求 `--yes`）。`--no-update` 完全跳过（离线/调试）。
//!
//! **更新不阻断登录**：版本检查失败、增量链走不通、非交互式无法确认，都只提示后
//! 继续登录；确认下载之后才发现游戏在运行（文件被占用）也只跳过更新。
//! 只有真正开始下载 / 打补丁之后的错误才中止（靠 `Progress::update_started` 区分）。
//! 例外是 `--check-update`：用户就是来查版本的，失败如实报错退出。

use std::path::Path;
use std::time::{Duration, Instant};

use patcher::download::{FileOutcome, Progress};
use patcher::version::UpdateDecision;

use crate::cli::{Args, UpdateMode};
use crate::consts::GAME_EXE;
use crate::error::{Error, Result};
use crate::ui;

/// 增量最多跳数。
const MAX_HOPS: usize = 32;

/// 终端进度（复用 `ui::status` 的原地刷新）。
struct TerminalProgress {
    last: Instant,
    last_pct: u64,
    /// 是否有一行未收尾的原地状态（决定 `file_end` 要不要补换行）。
    started: bool,
    /// 是否已真正开始改动本地文件（见 `Progress::update_started`）。
    update_started: bool,
}

impl TerminalProgress {
    fn new() -> Self {
        TerminalProgress {
            last: Instant::now(),
            last_pct: u64::MAX,
            started: false,
            update_started: false,
        }
    }

    fn end_line(&mut self) {
        if self.started {
            ui::status_end();
            self.started = false;
        }
    }
}

impl Progress for TerminalProgress {
    fn begin(&mut self, files: usize, bytes: u64) {
        if files > 0 {
            println!("待处理 {files} 个文件，约 {:.2} GB", bytes as f64 / 1e9);
        }
    }

    fn file_start(&mut self, path: &str, size: u64) {
        self.last = Instant::now();
        self.last_pct = u64::MAX;
        self.started = true;
        ui::status(&format!("  {path}  （{:.1} MB）", size as f64 / 1e6));
    }

    fn file_progress(&mut self, path: &str, done: u64, total: u64) {
        if self.last.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last = Instant::now();
        let pct = done.saturating_mul(100).checked_div(total).unwrap_or(0);
        if pct == self.last_pct {
            return;
        }
        self.last_pct = pct;
        self.started = true;
        ui::status(&format!(
            "  {path}  {pct}%  {:.1}/{:.1} MB",
            done as f64 / 1e6,
            total as f64 / 1e6
        ));
    }

    fn file_end(&mut self, path: &str, outcome: FileOutcome) {
        self.end_line();
        if outcome == FileOutcome::Downloaded {
            println!("  已下载 {path}");
        }
    }

    fn note(&mut self, msg: &str) {
        self.end_line();
        println!("{msg}");
    }

    fn update_started(&mut self) {
        self.update_started = true;
    }
}

/// 跑更新阶段。返回 `Ok(())` 表示可以继续登录。
pub fn run(args: &Args, root: &Path) -> Result<()> {
    let mode = args.update_mode();

    let cdn = args.cdn().map_err(Error::msg)?;
    let mut progress = TerminalProgress::new();
    // 口径来自 sdo-client：区服表路径与 CDN 版本/清单路径用的是同一个 appId/build id。
    let game_id = sdo_client::GAME_APP_ID.to_string();
    let build_id = sdo_client::BUILD_ID;

    match mode {
        UpdateMode::Skip => {
            proto::log::info("已按 --no-update 跳过更新阶段");
            Ok(())
        }

        // `--check-update`：用户就是来查版本的，失败要如实报错。
        UpdateMode::Check => {
            let outcome = patcher::check_update_with(root, &cdn, &game_id, build_id)
                .map_err(|e| Error::msg(e.to_string()))?;
            print_check(&outcome);
            Ok(())
        }

        UpdateMode::Full => {
            if !confirm_before_download(
                args,
                "将执行全量下载 / 校验：重新校验并补齐 game/ 下的所有文件（可能很大，请确保网络与磁盘空间充足）。",
                true,
            )? {
                return Ok(());
            }
            if let Some(hint) = game_running_hint() {
                println!("{hint}");
                return Ok(());
            }
            println!("全量下载 / 校验（可能很大，请耐心等待）…");
            let report = patcher::full_download_with(root, &cdn, &game_id, build_id, &mut progress)
                .map_err(|e| Error::msg(e.to_string()))?;
            println!(
                "完成：跳过 {}，下载 {}，共 {:.2} GB",
                report.skipped,
                report.downloaded,
                report.bytes as f64 / 1e9
            );
            Ok(())
        }

        UpdateMode::Auto => {
            let outcome = match patcher::check_update_with(root, &cdn, &game_id, build_id) {
                Ok(o) => o,
                Err(e) => {
                    // 版本检查失败不该拦住登录（CDN 抖动 / 403 / 证书问题都会走到这里）。
                    println!("版本检查失败，跳过更新直接登录：{e}");
                    println!("（如需彻底跳过更新阶段，可用 --no-update）");
                    proto::log::warn(&format!("版本检查失败，跳过更新直接登录：{e}"));
                    return Ok(());
                }
            };
            match outcome.decision {
                UpdateDecision::UpToDate => {
                    println!("已是最新版本：{}", outcome.remote.display);
                    // 已是最新：顺手清掉上次中断留下的临时目录。
                    cleanup_work(root);
                    Ok(())
                }
                UpdateDecision::LocalNewer => {
                    let local = outcome
                        .local
                        .as_ref()
                        .map(|l| l.display.as_str())
                        .unwrap_or("");
                    println!(
                        "本地版本比 CDN 还新（{local} > {}），跳过更新",
                        outcome.remote.display
                    );
                    Ok(())
                }
                UpdateDecision::UpdateAvailable => {
                    let local_display = outcome
                        .local
                        .as_ref()
                        .map(|l| l.display.as_str())
                        .unwrap_or("?");
                    let has_internal = outcome
                        .local
                        .as_ref()
                        .and_then(|l| l.internal.as_ref())
                        .is_some();

                    // 先预演增量链：链走不通（本地过旧 / CDN 无对应包）要在**下载之前**
                    // 就发现——既不该让用户白确认一次，更不该把登录拦下。
                    let chain = if has_internal {
                        match outcome.plan_incremental_chain(MAX_HOPS) {
                            Ok(c) => Some(c),
                            Err(e) => {
                                println!("无法规划增量更新（{e}），跳过更新直接登录。");
                                println!("（要强制校验 / 补齐，可用 --force-full）");
                                proto::log::warn(&format!("无法规划增量更新，跳过更新：{e}"));
                                return Ok(());
                            }
                        }
                    } else {
                        None
                    };
                    // 空链 = internal 已是最新（display 变了但内容没变）。
                    if matches!(&chain, Some(c) if c.is_empty()) {
                        println!("已是最新版本：{}", outcome.remote.display);
                        cleanup_work(root);
                        return Ok(());
                    }

                    let body = if chain.is_some() {
                        format!(
                            "发现新版本：{local_display} → {}（{}）\n将执行增量更新：逐跳打 delta，最后补齐缺失文件（不做全量校验）。",
                            outcome.remote.display, outcome.remote.name
                        )
                    } else {
                        "本地无 internal 版本信息（全新安装或只有 ffxivgame.ver）。\n将执行全量下载：校验并补齐 game/ 下的所有文件（可能很大）。"
                            .to_string()
                    };
                    // 自动更新：问不了就不问，跳过更新继续登录（`--force-full` 才强制要 `--yes`）。
                    if !confirm_before_download(args, &body, false)? {
                        return Ok(());
                    }
                    // 游戏在跑就只跳过更新（文件被占用），不拦登录。
                    if let Some(hint) = game_running_hint() {
                        println!("{hint}");
                        return Ok(());
                    }
                    if chain.is_some() {
                        println!(
                            "发现新版本：{local_display} → {}（{}）",
                            outcome.remote.display, outcome.remote.name
                        );
                        let report = match patcher::incremental_update_with(
                            root,
                            &cdn,
                            &game_id,
                            build_id,
                            MAX_HOPS,
                            &mut progress,
                        ) {
                            Ok(r) => r,
                            // 还没真正改动本地文件就失败：跳过更新继续登录
                            // （见模块头「更新不阻断登录」）。
                            Err(e) if !progress.update_started => {
                                println!("更新尚未开始就失败（{e}），跳过更新直接登录。");
                                proto::log::warn(&format!("增量更新未开始即失败，跳过：{e}"));
                                return Ok(());
                            }
                            Err(e) => return Err(Error::msg(e.to_string())),
                        };
                        println!(
                            "增量更新完成：{} 跳，delta 应用 {}（跳过 {}），补齐 {} 个文件",
                            report.hops, report.deltas_applied, report.deltas_skipped, report.repaired
                        );
                    } else {
                        println!(
                            "本地无 internal 版本信息（全新安装或只有 ffxivgame.ver），走全量下载…"
                        );
                        let report =
                            patcher::full_download_with(root, &cdn, &game_id, build_id, &mut progress)
                                .map_err(|e| Error::msg(e.to_string()))?;
                        println!(
                            "全量完成：跳过 {}，下载 {}，共 {:.2} GB",
                            report.skipped,
                            report.downloaded,
                            report.bytes as f64 / 1e9
                        );
                    }
                    Ok(())
                }
            }
        }
    }
}

/// 游戏在运行 → 给出提示（更新要写游戏文件，被占用会失败）；否则 `None`。
///
/// 只在**确认要下载之后**才问，免得「本来就不用更新」也被拦下来。
fn game_running_hint() -> Option<String> {
    crate::proc::is_running(GAME_EXE).then(|| {
        format!(
            "检测到 {GAME_EXE} 正在运行，跳过本次更新（文件被占用）；如需更新请先关闭游戏。"
        )
    })
}

/// 下载前的确认闸门：`--yes` 直接放行；交互式问一次；问不了（stdin 非终端）时
/// 看 `required`：
///
/// - `required = true`（用户显式要求，如 `--force-full`）：报错，要求 `--yes`；
/// - `required = false`（自动更新）：跳过本次更新继续登录，不让确认把登录拦下。
///
/// 回答 n 等价于 `--no-update`：跳过本次更新，继续登录。
fn confirm_before_download(args: &Args, body: &str, required: bool) -> Result<bool> {
    if args.yes {
        println!("已按 --yes 跳过确认。");
        return Ok(true);
    }
    match ui::confirm_update(body) {
        Some(true) => Ok(true),
        Some(false) => {
            println!("已选择暂不更新，跳过更新阶段直接登录（版本可能与服务器不一致）。");
            Ok(false)
        }
        None if required => Err(Error::msg(
            "非交互式运行，无法确认全量下载；确认请加 --yes，跳过更新请加 --no-update",
        )),
        None => {
            println!("非交互式运行，无法确认更新；跳过本次更新直接登录（要更新请加 --yes）。");
            Ok(false)
        }
    }
}

/// 删除临时目录 `<root>/_update`：更新已完成、且（最好）游戏已成功启动后调用。
pub fn cleanup_work(root: &Path) {
    let work = root.join("_update");
    if work.exists() {
        if let Err(e) = std::fs::remove_dir_all(&work) {
            proto::log::debug(&format!("清理 {} 失败：{e}", work.display()));
        }
    }
}

fn print_check(outcome: &patcher::CheckOutcome) {
    println!(
        "CDN 最新：{}（{}）",
        outcome.remote.display, outcome.remote.name
    );
    match &outcome.local {
        Some(l) => println!(
            "本地版本：{}（internal={:?}，来源 {:?}）",
            l.display, l.internal, l.source
        ),
        None => println!("本地版本：无（全新安装）"),
    }
    let text = match outcome.decision {
        UpdateDecision::UpToDate => "已是最新",
        UpdateDecision::UpdateAvailable => "有更新可用",
        UpdateDecision::LocalNewer => "本地比 CDN 新",
    };
    println!("结论：{text}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::io::IsTerminal;

    /// `patcher` 侧只给示例用的默认值必须与 `sdo-client` 的口径一致，
    /// 否则区服表能拉、CDN 却 404（或反过来）。
    #[test]
    fn patcher_defaults_match_sdo_client() {
        assert_eq!(patcher::cdn::GAME_ID, sdo_client::GAME_APP_ID.to_string());
        assert_eq!(patcher::cdn::BUILD_ID, sdo_client::BUILD_ID);
    }

    /// 非交互式（stdin 不是终端）：自动更新跳过并继续登录，`--force-full` 要求 `--yes`。
    /// 交互式跑 `cargo test` 时 stdin 是终端，跳过断言（否则会等输入）。
    #[test]
    fn non_interactive_confirm_does_not_block_login() {
        if std::io::stdin().is_terminal() {
            eprintln!("跳过：stdin 是终端");
            return;
        }
        let auto = Args::parse_from(["x"]);
        assert!(!confirm_before_download(&auto, "b", false).unwrap());
        assert!(confirm_before_download(&auto, "b", true).is_err());

        // `--yes` 下两条路径都直接放行。
        let yes = Args::parse_from(["x", "--yes"]);
        assert!(confirm_before_download(&yes, "b", false).unwrap());
        assert!(confirm_before_download(&yes, "b", true).unwrap());
    }
}
