//! 游戏进程启动。
//!
//! Windows 上直接跑 exe；其他平台要先经兼容层（wine / umu-run / 自定义命令）跑同一个 PE。
//! 命令行转义、工作目录、句柄回收都交给 `std::process::Command`。
//!
//! 交接环境变量（`SDO_FFXIV_*`）用 `Command::envs` 直接写进子进程的环境块，不改启动器
//! 自己的进程环境；其余变量（PATH 等）照常继承。非 Windows 时兼容层把子进程环境映射进
//! Windows 进程的环境块，游戏侧 DLL 照常读得到。

use std::ffi::OsString;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::{Child, Command};

use proto::consts::{ENV_AREAID, ENV_BASE, ENV_SNDAID, ENV_TICKET};
use proto::log;

/// `--run-via` 的环境变量等价形式。
const ENV_RUN_VIA: &str = "SDO_FFXIV_RUN_VIA";

/// 启动游戏用的兼容层命令：程序 + 前置参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launcher {
    program: OsString,
    prefix: Vec<OsString>,
}

impl Launcher {
    /// 解析取值：程序 + 可选参数，按空白拆分。
    /// 参数里必须带空格时，请写一个包装脚本再指过来。
    pub fn parse(spec: &str) -> Result<Launcher, String> {
        let mut parts = spec.split_whitespace();
        let program = parts
            .next()
            .ok_or_else(|| "--run-via 不能为空".to_string())?;
        Ok(Launcher {
            program: program.into(),
            prefix: parts.map(OsString::from).collect(),
        })
    }
}

impl std::fmt::Display for Launcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.program.to_string_lossy())?;
        for a in &self.prefix {
            write!(f, " {}", a.to_string_lossy())?;
        }
        Ok(())
    }
}

/// 解析该用什么启动游戏：`--run-via` > `SDO_FFXIV_RUN_VIA` > 平台默认。
///
/// 返回 `None` 表示直接运行 exe（Windows 的默认行为）。
pub fn resolve_launcher(cli: Option<&str>) -> Result<Option<Launcher>, String> {
    if let Some(spec) = cli.map(str::trim).filter(|s| !s.is_empty()) {
        return Launcher::parse(spec).map(Some);
    }
    if let Ok(spec) = std::env::var(ENV_RUN_VIA) {
        let spec = spec.trim();
        if !spec.is_empty() {
            return Launcher::parse(spec).map(Some);
        }
    }
    detect()
}

/// Windows：exe 能直接跑，不需要兼容层。
#[cfg(windows)]
fn detect() -> Result<Option<Launcher>, String> {
    Ok(None)
}

/// Unix：按 `wine` → `umu-run` 顺序在 `PATH` 里探测。
///
/// 不去猜 `proton` / `flatpak`：它们要么不在 `PATH` 里，要么在 `PATH` 里但还缺一堆
/// `STEAM_COMPAT_*` 环境变量，探测到也用不了——那种情况请显式 `--run-via`。
#[cfg(unix)]
fn detect() -> Result<Option<Launcher>, String> {
    for name in ["wine", "umu-run"] {
        if let Some(path) = which(name) {
            return Ok(Some(Launcher {
                program: path.into_os_string(),
                prefix: Vec::new(),
            }));
        }
    }
    Err(format!(
        "非 Windows 平台需要兼容层才能跑 Windows 版游戏：PATH 里没找到 wine 或 umu-run。\n\
         装好 wine，或用 --run-via <命令> / {ENV_RUN_VIA} 指定，例如：\n\
         --run-via \"flatpak run org.winehq.Wine\""
    ))
}

#[cfg(not(any(windows, unix)))]
fn detect() -> Result<Option<Launcher>, String> {
    Err(format!(
        "当前平台未知，请用 --run-via <命令> / {ENV_RUN_VIA} 指定怎么启动游戏"
    ))
}

/// 在 `PATH` 里找可执行文件（不引入依赖）。
#[cfg(unix)]
fn which(name: &str) -> Option<PathBuf> {
    which_in(name, std::env::var_os("PATH").as_deref())
}

#[cfg(unix)]
fn which_in(name: &str, paths: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    std::env::split_paths(paths?)
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.is_file()
        && p.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// 交接给游戏进程的环境变量（游戏侧 DLL 只读）。
///
/// 由 `Command::envs` 直接写进子进程的环境块，不再改启动器自己的进程环境。
#[derive(Debug, Clone, Copy)]
pub struct Delivery<'a> {
    pub ticket: &'a str,
    pub snda_id: &'a str,
    pub area_id: &'a str,
    pub base: &'a str,
}

impl Delivery<'_> {
    /// `(名字, 值)` 列表，直接喂给 `Command::envs`。
    fn pairs(&self) -> [(&'static str, &str); 4] {
        [
            (ENV_TICKET, self.ticket),
            (ENV_SNDAID, self.snda_id),
            (ENV_AREAID, self.area_id),
            (ENV_BASE, self.base),
        ]
    }

    /// 交接内容自检：票据非空、`base` 与 `areaId` 一致。
    ///
    /// 原先还要比对"写进环境的值与内存值是否一致"：现在环境由 `Command::envs` 直接交给
    /// 子进程，不存在写丢的可能，那部分校验已经没有意义。
    fn validate(&self) -> Result<(), String> {
        if self.ticket.is_empty() || self.snda_id.is_empty() {
            return Err("票据异常：请重试".to_string());
        }
        if !self.base.contains(&format!("-AreaID={} ", self.area_id)) {
            return Err("大区参数不一致，请重试".to_string());
        }
        Ok(())
    }
}

/// 启动游戏，返回子进程句柄。
pub fn launch(
    exe: &Path,
    game_dir: &Path,
    delivery: &Delivery<'_>,
    launcher: Option<&Launcher>,
) -> Result<Child, String> {
    delivery.validate()?;
    if !exe.is_file() {
        return Err(format!("游戏可执行文件不存在：{}", exe.display()));
    }
    if !game_dir.is_dir() {
        return Err(format!("游戏工作目录不存在：{}", game_dir.display()));
    }
    // 兼容层形态：`<兼容层命令> <前置参数…> <exe> <游戏参数…>`；没有兼容层就直接跑 exe。
    let mut cmd = match launcher {
        Some(l) => {
            let mut c = Command::new(&l.program);
            c.args(&l.prefix).arg(exe);
            c
        }
        None => Command::new(exe),
    };
    cmd.current_dir(game_dir);
    // 只往子进程环境里"加"这四项，其余（PATH 等）照旧继承——兼容层也要靠 PATH。
    cmd.envs(delivery.pairs());
    // `base` 形如 `-AppID=… -AreaID=… Dev.LobbyHost01=…`，每个 token 内部不含空白，
    // 按空白拆成独立参数，与原先手工拼命令行、再由 CRT 分词的结果一致。
    cmd.args(delivery.base.split_whitespace());
    cmd.spawn().map_err(|e| {
        log::debug(&format!("启动游戏失败细节：{e}"));
        let via = match launcher {
            Some(l) => format!("（启动命令：{l}）"),
            None => String::new(),
        };
        format!(
            "启动游戏失败（{e}）{via}，请检查游戏路径与权限：{}",
            exe.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_program_and_prefix() {
        let l = Launcher::parse("flatpak run org.winehq.Wine").unwrap();
        assert_eq!(l.program, OsString::from("flatpak"));
        assert_eq!(
            l.prefix,
            vec![OsString::from("run"), OsString::from("org.winehq.Wine")]
        );
        assert_eq!(l.to_string(), "flatpak run org.winehq.Wine");

        let plain = Launcher::parse("  wine  ").unwrap();
        assert_eq!(plain.program, OsString::from("wine"));
        assert!(plain.prefix.is_empty());

        assert!(Launcher::parse("   ").is_err());
        assert!(Launcher::parse("").is_err());
    }

    /// 显式指定优先于平台探测（不碰 PATH）。
    #[test]
    fn cli_override_wins() {
        let l = resolve_launcher(Some(" mylayer --flag ")).unwrap().unwrap();
        assert_eq!(l.to_string(), "mylayer --flag");
    }

    /// 用一个假兼容层脚本记录 argv，验证 `<兼容层> <exe> <游戏参数…>` 的真实拼接。
    #[cfg(unix)]
    #[test]
    fn compat_layer_gets_exe_then_game_args() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("xivtl-launch-{}", std::process::id()));
        let game_dir = dir.join("game");
        std::fs::create_dir_all(&game_dir).unwrap();
        let exe = game_dir.join("ffxiv_dx11.exe");
        std::fs::write(&exe, b"").unwrap();

        let out = dir.join("argv.txt");
        let script = dir.join("fake-wine.sh");
        // 假兼容层把收到的 argv 与 `SDO_FFXIV_*` 环境变量都落到文件。
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n{{ \\\n  echo ARGV; \\\n  printf '%s\\n' \"$@\"; \\\n  echo ENV; \\\n  env | grep '^SDO_FFXIV_' | sort; \\\n}} > {}\n",
                out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let layer = Launcher::parse(script.to_str().unwrap()).unwrap();
        let delivery = Delivery {
            ticket: "ULS21-TESTTICKET",
            snda_id: "1234567890",
            area_id: "7",
            base: "-AppID=100001900 -AreaID=7 Dev.LobbyHost01=ffxivlobby07.ff14.sdo.com",
        };
        let mut child = launch(&exe, &game_dir, &delivery, Some(&layer)).unwrap();
        assert!(child.wait().unwrap().success());

        let recorded = std::fs::read_to_string(&out).unwrap();
        let mut lines = recorded.lines();
        assert_eq!(lines.next(), Some("ARGV"));
        let argv: Vec<&str> = lines.by_ref().take_while(|l| *l != "ENV").collect();
        let envs: Vec<&str> = lines.collect();

        assert_eq!(
            argv,
            vec![
                exe.to_str().unwrap(),
                "-AppID=100001900",
                "-AreaID=7",
                "Dev.LobbyHost01=ffxivlobby07.ff14.sdo.com"
            ],
            "兼容层应收到 exe 在前、游戏参数在后"
        );
        // 交接环境变量必须真的进了子进程（游戏侧 DLL 靠它拿票据）。
        for want in [
            "SDO_FFXIV_TICKET=ULS21-TESTTICKET",
            "SDO_FFXIV_SNDAID=1234567890",
            "SDO_FFXIV_AREAID=7",
            "SDO_FFXIV_BASE=-AppID=100001900 -AreaID=7 Dev.LobbyHost01=ffxivlobby07.ff14.sdo.com",
        ] {
            assert!(
                envs.contains(&want),
                "子进程环境缺少 {want}；实际：{envs:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delivery_validate_rules() {
        let ok = Delivery {
            ticket: "T",
            snda_id: "S",
            area_id: "7",
            base: "-AppID=100001900 -AreaID=7 Dev.LobbyHost01=h",
        };
        assert!(ok.validate().is_ok());

        let no_ticket = Delivery { ticket: "", ..ok };
        assert!(no_ticket.validate().is_err());

        let no_snda = Delivery { snda_id: "", ..ok };
        assert!(no_snda.validate().is_err());

        // base 里的 AreaID 与 area_id 不一致：必须拦住。
        let mismatch = Delivery { area_id: "8", ..ok };
        assert!(mismatch.validate().is_err());

        // `-AreaID=70` 不能被当成 `-AreaID=7`。
        let prefix_trap = Delivery {
            base: "-AppID=100001900 -AreaID=70 Dev.LobbyHost01=h",
            ..ok
        };
        assert!(prefix_trap.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn which_requires_executable_bit() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("xivtl-which-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("wine");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();

        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            which_in("wine", Some(dir.as_os_str())).is_none(),
            "无执行位不算"
        );

        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("wine", Some(dir.as_os_str())), Some(fake.clone()));
        assert!(which_in("umu-run", Some(dir.as_os_str())).is_none());
        assert!(which_in("wine", None).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
