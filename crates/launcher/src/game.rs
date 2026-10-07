//! 游戏目录定位与登录 DLL 落点校验。

use std::path::{Path, PathBuf};

use proto::consts::{DLL_BUILD_MARKER, DLL_NAME};

use crate::consts::{GAME_EXE, GAME_SUBDIR};
use proto::log;

#[derive(Debug, Clone)]
pub struct GameDirs {
    pub game_dir: PathBuf,
    pub exe: PathBuf,
}

impl GameDirs {
    fn from_root(root: &Path) -> Option<GameDirs> {
        let game_dir = root.join(GAME_SUBDIR);
        let exe = game_dir.join(GAME_EXE);
        if exe.is_file() {
            Some(GameDirs { game_dir, exe })
        } else {
            None
        }
    }
}

/// `--game-dir` 的起点：绝对路径原样，相对路径接当前目录，缺省用 EXE 所在目录。
fn start_dir(arg: Option<&Path>) -> PathBuf {
    match arg {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => std::env::current_dir().unwrap_or_default().join(p),
        None => std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
    }
}

/// 定位游戏目录；失败返回可读错误。
pub fn resolve(arg: Option<&Path>) -> Result<GameDirs, String> {
    let root = resolve_root(arg);
    if let Some(g) = GameDirs::from_root(&root) {
        return Ok(g);
    }
    let candidate = root.join(GAME_SUBDIR);
    if candidate.is_dir() {
        return Err(format!(
            "找到 {} 但缺少 {GAME_EXE}，请用 --game-dir 指定正确的安装根",
            candidate.display()
        ));
    }
    Err(match arg {
        // 显式传参不会上溯，所以提示里也不提「上溯」。
        Some(_) => format!(
            "{} 下没有 {GAME_SUBDIR}/{GAME_EXE}，请用 --game-dir 指定安装根或 game 目录",
            root.display()
        ),
        None => format!(
            "从 {} 上溯未找到含 {GAME_SUBDIR}/{GAME_EXE} 的安装根，请用 --game-dir 指定",
            root.display()
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_dir_reports_error() {
        let missing = std::env::temp_dir().join("xivtl-dir-that-does-not-exist");
        let err = resolve(Some(&missing)).unwrap_err();
        assert!(err.contains("--game-dir"), "{err}");
    }

    /// 显式 `--game-dir` 不上溯：祖先里有 `game/` 也不能抢走落点。
    #[test]
    fn resolve_root_does_not_walk_up_for_explicit_dir() {
        let base = std::env::temp_dir().join(format!("xivtl-root-{}", std::process::id()));
        let outer_game = base.join("game");
        std::fs::create_dir_all(&outer_game).unwrap();
        let fresh = base.join("FFXIV-new");
        std::fs::create_dir_all(&fresh).unwrap();

        // 全新安装的目标目录：不能被 <base>/game 劫持。
        assert_eq!(resolve_root(Some(&fresh)), fresh);
        // 指到 `<root>/game` 退回上一层。
        assert_eq!(resolve_root(Some(&outer_game)), base);
        // 指到 `<root>/game/ffxiv_dx11.exe` 也退回 <root>。
        let exe = outer_game.join(GAME_EXE);
        std::fs::write(&exe, b"x").unwrap();
        assert_eq!(resolve_root(Some(&exe)), base);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// 无参数时才上溯；这里只能直接测「给了参数就认参数」。
    /// （无参数分支依赖 `current_exe()`，只能在集成环境里验证。）
    #[test]
    fn resolve_root_trusts_explicit_dir() {
        let base = std::env::temp_dir().join(format!("xivtl-up-{}", std::process::id()));
        let tools = base.join("tools");
        std::fs::create_dir_all(base.join("game")).unwrap();
        std::fs::create_dir_all(&tools).unwrap();
        assert_eq!(resolve_root(Some(&tools)), tools);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn login_dll_path_matches_game_load_path() {
        let game_dir = Path::new("ffxiv").join("game");
        assert_eq!(
            login_dll_path(&game_dir),
            PathBuf::from("ffxiv")
                .join("sdo")
                .join("sdologin")
                .join(DLL_NAME)
        );
    }

    #[test]
    fn dll_check_detects_our_marker() {
        let dir = std::env::temp_dir().join(format!("xivtl-dll-{}", std::process::id()));
        let game = dir.join("game");
        let sdo = dir.join("sdo").join("sdologin");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(&sdo).unwrap();
        let ours = sdo.join(DLL_NAME);
        std::fs::write(&ours, format!("junk{}junk", DLL_BUILD_MARKER)).unwrap();
        let check = check_login_dll(&game);
        assert!(check.present);
        assert_eq!(check.path, ours);
        assert!(check.is_ours);

        std::fs::write(&ours, "official-ish bytes").unwrap();
        let check2 = check_login_dll(&game);
        assert!(check2.present);
        assert!(!check2.is_ours);

        // 删除 → 找不到，且 verify 必须报错（含实际加载路径）
        std::fs::remove_file(&ours).unwrap();
        let err = verify_login_dll(&game, false).unwrap_err();
        assert!(err.contains("sdologinentry64.dll"), "{err}");
        // skip 时只告警不报错
        assert!(verify_login_dll(&game, true).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 定位安装根（含 `game/` 的目录）。
///
/// 与 [`resolve`] 不同，**不要求** `game/ffxiv_dx11.exe` 已存在——全新安装时
/// 根目录下还没有 `game/`，更新阶段需要一个可写的落点。
///
/// - 显式 `--game-dir`：只认它本身（它指到 `game/` 或 exe 时退回上一层），
///   **不上溯**——否则「装到别的目录」会被祖先目录里的 `game/` 劫持。
/// - 无参数：从 EXE 所在目录上溯找含 `game/` 的目录，找不到就用 EXE 目录。
pub fn resolve_root(arg: Option<&Path>) -> PathBuf {
    let start = start_dir(arg);
    // 指到 exe 上就退到它所在目录（那通常是 `game/`）。
    let start = if start.is_file() {
        start.parent().map(Path::to_path_buf).unwrap_or(start)
    } else {
        start
    };

    if arg.is_some() {
        // `--game-dir <root>/game` → 用 <root>。
        if start.file_name().map(|n| n == GAME_SUBDIR).unwrap_or(false) {
            return start.parent().map(Path::to_path_buf).unwrap_or(start);
        }
        return start;
    }

    for cand in start.ancestors() {
        if cand.join(GAME_SUBDIR).is_dir() {
            return cand.to_path_buf();
        }
        // `cand` 自己就是 `game/`。
        if cand.join(GAME_EXE).is_file() {
            return cand.parent().unwrap_or(cand).to_path_buf();
        }
    }
    start
}

/// 游戏实际读取登录 DLL 的路径。
pub fn login_dll_path(game_dir: &Path) -> PathBuf {
    let root = game_dir.parent().unwrap_or(game_dir);
    root.join("sdo").join("sdologin").join(DLL_NAME)
}

#[derive(Debug, Clone)]
pub struct DllCheck {
    pub path: PathBuf,
    pub present: bool,
    pub is_ours: bool,
}

/// 目标文件是否含自研构建标记（`dllswap` 复用，逻辑唯一）。
pub(crate) fn dll_has_marker(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => bytes
            .windows(DLL_BUILD_MARKER.len())
            .any(|w| w == DLL_BUILD_MARKER.as_bytes()),
        Err(_) => false,
    }
}

pub fn check_login_dll(game_dir: &Path) -> DllCheck {
    let path = login_dll_path(game_dir);
    let present = path.is_file();
    DllCheck {
        is_ours: present && dll_has_marker(&path),
        path,
        present,
    }
}

/// 校验 DLL（内部会打印结果）。
pub fn verify_login_dll(game_dir: &Path, skip: bool) -> Result<DllCheck, String> {
    let check = check_login_dll(game_dir);
    match (check.present, check.is_ours) {
        (true, true) => {
            println!("登录 DLL：{}", check.path.display());
            Ok(check)
        }
        (true, false) => {
            println!(
                "注意：{} 不是本启动器的登录组件，可能会登录失败。",
                check.path.display(),
            );
            Ok(check)
        }
        (false, _) => {
            let msg = format!(
                "未找到登录组件 {DLL_NAME}。\n\
                 请将 {DLL_NAME} 复制到 {}。",
                check.path.display()
            );
            if skip {
                log::warn(&msg);
                Ok(check)
            } else {
                Err(msg)
            }
        }
    }
}
