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

    fn from_game_dir(game_dir: &Path) -> Option<GameDirs> {
        let exe = game_dir.join(GAME_EXE);
        if exe.is_file() {
            Some(GameDirs {
                game_dir: game_dir.to_path_buf(),
                exe,
            })
        } else {
            None
        }
    }
}

/// 定位游戏目录；失败返回可读错误。
pub fn resolve(arg: Option<&Path>) -> Result<GameDirs, String> {
    let start: PathBuf = match arg {
        Some(p) => {
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                std::env::current_dir().unwrap_or_default().join(p)
            }
        }
        None => std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default()),
    };

    if start.file_name().map(|n| n == GAME_SUBDIR).unwrap_or(false) {
        if let Some(g) = GameDirs::from_game_dir(&start) {
            return Ok(g);
        }
    }
    for cand in start.ancestors() {
        if let Some(g) = GameDirs::from_root(cand) {
            return Ok(g);
        }
    }
    let root_candidate = start.join(GAME_SUBDIR);
    if root_candidate.is_dir() {
        return Err(format!(
            "找到 {} 但缺少 {GAME_EXE}，请用 --game-dir 指定正确的安装根",
            root_candidate.display()
        ));
    }
    Err(format!(
        "从 {} 上溯未找到含 {GAME_SUBDIR}/{GAME_EXE} 的安装根，请用 --game-dir 指定",
        start.display()
    ))
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

fn has_build_marker(path: &Path) -> bool {
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
        is_ours: present && has_build_marker(&path),
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
                 游戏实际加载路径：{}\n\
                 把 {DLL_NAME} 复制到上述目录即可。",
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
