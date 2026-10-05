use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};


#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Mode {
    Auto,
    Qr,
    Push,
}

/// 终端二维码的渲染方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum QrRender {
    Auto,
    Ascii,
}

#[derive(Debug, Clone, Parser)]
#[command(
    name = "sdo-ffxiv-launcher",
    version,
    about = "FFXIV CN（盛趣）自研启动器：纯 CLI + 终端二维码",
    disable_help_subcommand = true
)]
pub struct Args {
    /// 安装根或 `game/` 目录；缺省自动查找
    #[arg(long, value_name = "path")]
    pub game_dir: Option<PathBuf>,

    /// 选区 id；缺省用上次记住的大区，都没有则进菜单
    #[arg(long, value_name = "id")]
    pub area: Option<String>,

    /// 首包不带保持登录（中途可按 k 开启）
    #[arg(long = "no-keep-login", action = ArgAction::SetTrue)]
    pub no_keep_login: bool,

    /// 登录模式
    #[arg(long, value_enum, default_value_t = Mode::Auto)]
    pub mode: Mode,

    /// push 专用账号（邮箱/手机）
    #[arg(long, value_name = "账号")]
    pub account: Option<String>,

    /// 单张二维码倒计时秒数
    #[arg(long, default_value_t = sdo_client::Policy::default().code_timeout_secs)]
    pub qr_timeout: u64,

    /// 单张二维码最大轮询次数
    #[arg(long, default_value_t = sdo_client::Policy::default().max_attempts)]
    pub qr_max_attempts: u32,

    /// 轮询间隔下限（毫秒）
    #[arg(long, default_value_t = sdo_client::Policy::default().poll_min_ms)]
    pub poll_min_ms: u64,

    /// 轮询间隔上限（毫秒）
    #[arg(long, default_value_t = sdo_client::Policy::default().poll_max_ms)]
    pub poll_max_ms: u64,

    /// 常驻：等待游戏退出后清理登录信息；默认启动成功即退出
    #[arg(long, action = ArgAction::SetTrue)]
    pub stay: bool,

    /// 用哪个命令启动游戏（非 Windows 平台缺省自动探测 wine / umu-run）
    ///
    /// 例：--run-via "flatpak run org.winehq.Wine"；也可用环境变量 SDO_FFXIV_RUN_VIA
    #[arg(long = "run-via", value_name = "cmd")]
    pub run_via: Option<String>,

    /// 手动指定本机 IP（默认自动获取）
    #[arg(long, value_name = "ip")]
    pub ep_ip: Option<String>,

    /// 手动指定机器名（默认自动生成）
    #[arg(long, value_name = "name")]
    pub ep_name: Option<String>,

    /// 手动指定机器标识（默认自动生成）
    #[arg(long, value_name = "mac")]
    pub mac_id: Option<String>,

    /// 只跑自检后退出（不启动游戏）
    #[arg(long = "self-check", action = ArgAction::SetTrue)]
    pub self_check: bool,

    /// 跳过登录组件检查
    #[arg(long = "skip-dll-check", action = ArgAction::SetTrue)]
    pub skip_dll_check: bool,

    /// 日志文件路径。
    #[arg(long, value_name = "path")]
    pub log_file: Option<PathBuf>,

    /// 终端二维码渲染方式
    #[arg(long = "qr-render", value_enum, default_value_t = QrRender::Auto)]
    pub qr_render: QrRender,

    /// 二维码图片的保存目录
    #[arg(long = "qr-out", value_name = "dir")]
    pub qr_out: Option<PathBuf>,
}

impl Args {
    /// 生效的手机确认账号：`--account` 优先，否则用上次记住的。
    pub fn account_or(&self, stored: Option<&str>) -> Option<String> {
        self.account
            .clone()
            .or_else(|| stored.map(str::to_string))
            .filter(|a| !a.trim().is_empty())
    }

    /// 本次怎么登录。
    ///
    /// `last` 是 `device.json` 记的上次用的链（auto 用它当回退）；`stored_account` 是
    /// 上次记住的手机确认账号（`--account` 缺省时用它）。
    pub fn method(
        &self,
        last: sdo_client::Chain,
        stored_account: Option<&str>,
    ) -> Result<sdo_client::Method, String> {
        let (chain, fast) = match self.mode {
            Mode::Qr => (sdo_client::Chain::Qr, false),
            Mode::Auto => (last, true),
            Mode::Push => (
                sdo_client::Chain::Push {
                    account: self
                        .account_or(stored_account)
                        .ok_or("--mode push 需要 --account（或先成功用过一次手机登录）")?,
                },
                false,
            ),
        };
        Ok(sdo_client::Method { chain, fast })
    }

    /// 构造流程调参；未在 CLI 暴露的项用默认值。
    pub fn policy(&self) -> sdo_client::Policy {
        sdo_client::Policy {
            code_timeout_secs: self.qr_timeout,
            max_attempts: self.qr_max_attempts,
            poll_min_ms: self.poll_min_ms,
            poll_max_ms: self.poll_max_ms,
            ..sdo_client::Policy::default()
        }
    }

    /// 首包是否勾选保持登录。
    pub fn keep_login(&self) -> bool {
        !self.no_keep_login
    }

    /// 参数合法性自检（不触网）。
    pub fn validate(&self) -> Result<(), String> {
        if self.poll_min_ms == 0 || self.poll_max_ms == 0 || self.poll_min_ms > self.poll_max_ms {
            return Err("--poll-min-ms/--poll-max-ms 非法".into());
        }
        if self.qr_max_attempts == 0 {
            return Err("--qr-max-attempts 必须大于 0".into());
        }
        if self.qr_timeout == 0 {
            return Err("--qr-timeout 必须大于 0".into());
        }
        if self.qr_max_attempts < 5 || self.qr_timeout < 15 {
            eprintln!("提示：轮询参数偏小，未扫码时会很快换码");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn keep_login_defaults_to_checked() {
        let a = Args::parse_from(["sdo-ffxiv-launcher"]);
        assert!(a.keep_login(), "默认勾选");
        assert_eq!(a.mode, Mode::Auto);
        assert_eq!(a.qr_timeout, 120);
        assert_eq!(a.qr_max_attempts, 60);
        assert_eq!(a.poll_min_ms, 900);
        assert_eq!(a.poll_max_ms, 1100);
        assert!(!a.stay);
    }

    #[test]
    fn no_keep_login_is_unchecked() {
        let a = Args::parse_from(["sdo-ffxiv-launcher", "--no-keep-login"]);
        assert!(!a.keep_login());
    }

    #[test]
    fn push_requires_account() {
        let a = Args::parse_from(["sdo-ffxiv-launcher", "--mode", "push"]);
        assert!(a.method(sdo_client::Chain::Qr, None).is_err());
        let b = Args::parse_from(["sdo-ffxiv-launcher", "--mode", "push", "--account", "a@b.c"]);
        assert!(b.method(sdo_client::Chain::Qr, None).is_ok());
        // 上次记住的账号也算
        assert!(a.method(sdo_client::Chain::Qr, Some("a@b.c")).is_ok());
    }

    #[test]
    fn poll_window_validated() {
        let a = Args::parse_from([
            "sdo-ffxiv-launcher",
            "--poll-min-ms",
            "1200",
            "--poll-max-ms",
            "1000",
        ]);
        assert!(a.validate().is_err());
    }
}
