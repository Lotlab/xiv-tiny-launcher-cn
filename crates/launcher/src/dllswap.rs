//! 瞬时替换（默认开，`--no-swap-dll` 关）：启动前换上自研 DLL，游戏加载完换回官方。
//!
//! 两条实测结论：已加载的 DLL 禁止覆盖/删除但允许同目录改名（改名后原路径可立即放回
//! 官方，模块靠句柄继续跑）；游戏在 worker 线程里 `LoadLibraryA` 成功后依次调
//! `Login[9] SetLoginMode`、`Login[3] SetHWND`、`Login[10] GetTicket` 且成功路径永不
//! `FreeLibrary`（反编译见 `ffxiv_dx11.exe` 的 `FUN_14005e000/14005e280/14005ce30`），
//! 所以确认加载后换回官方就是安全的。
//!
//! 加载判据（二者任一）：marker 文件（主信号，跨平台，见 `proto::swap_marker`）或
//! 进程模块表（Windows 补充信号，老 DLL 也能工作）。
//! 自研来源：`--ours-dll` > 启动器旁 sidecar > 编进 exe 的内嵌副本（见 `build.rs`）。
//!
//! 流程：`prepare`（拿票后换上）→ `launch` → `wait_game_loaded` → `restore`（换回官方）
//! → `cleanup`（游戏退出后）。失败一律 fail-safe 留自研在位，只记日志，不拦启动。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use proto::consts::DLL_NAME;
use proto::{log, swap_marker};

use crate::game;

/// 自研 DLL 在启动器旁边的存放名（sidecar；避免与游戏加载名混淆）。
pub const OURS_FILE_NAME: &str = "sdologinentry64.ours.dll";
/// 官方备份名（内容不一致且已有备份时改用带 pid 后缀，永不覆盖用户数据）。
pub const OFFICIAL_BAK_NAME: &str = "sdologinentry64.dll.official.bak";
/// 藏起已加载自研 DLL 的文件名前缀（后缀为起本次游戏的 launcher pid，避免双开互踩；
/// 启动器有单实例锁，正常只会有一个活着）。
const IN_USE_PREFIX: &str = "sdologinentry64.dll.in_use.";
/// 等待游戏加载的最长时限（主窗口建完后 worker 线程才加载，机械硬盘上也该够了）。
pub const WAIT_TIMEOUT: Duration = Duration::from_secs(60);
const WAIT_POLL: Duration = Duration::from_millis(100);

/// 编进 exe 的自研 DLL（构建时 `SDOLOGINENTRY64_DLL` 指向的产物，见 `build.rs`）。
#[cfg(embedded_dll)]
const EMBEDDED_DLL_BYTES: &[u8] = include_bytes!(concat!(env!("EMBEDDED_DLL_PATH")));

/// 内嵌副本是否存在（构建时决定）。
pub fn has_embedded_dll() -> bool {
    cfg!(embedded_dll)
}

/// 自研 DLL 的来源。
#[derive(Debug, Clone)]
pub enum OursSource {
    /// 外部文件（`--ours-dll` 或 sidecar）。
    File(PathBuf),
    /// 编进 exe 的字节（零附带文件模式）。
    Embedded,
}

impl OursSource {
    /// 把自研 DLL 落到 `dst`（File=复制，Embedded=写出；写完后由调用方验标记）。
    fn place(&self, dst: &Path) -> Result<(), String> {
        match self {
            OursSource::File(src) => {
                if !src.is_file() {
                    return Err(format!(
                        "找不到自研 DLL：{}（用 --ours-dll 指定，或确认安装时已部署 {OURS_FILE_NAME}，或用内嵌版本构建）",
                        src.display()
                    ));
                }
                std::fs::copy(src, dst)
                    .map_err(|e| format!("复制自研 DLL 到 {} 失败：{e}", dst.display()))?;
                Ok(())
            }
            OursSource::Embedded => write_embedded(dst),
        }
    }
}

/// 写出编进 exe 的 DLL（构建时未内嵌则报错）。
#[cfg(embedded_dll)]
fn write_embedded(dst: &Path) -> Result<(), String> {
    if EMBEDDED_DLL_BYTES.is_empty() {
        return Err("内嵌 DLL 为空".to_string());
    }
    std::fs::write(dst, EMBEDDED_DLL_BYTES)
        .map_err(|e| format!("写出内嵌 DLL 到 {} 失败：{e}", dst.display()))?;
    Ok(())
}

/// 构建时未内嵌 DLL（sidecar/`--ours-dll` 模式）。
#[cfg(not(embedded_dll))]
fn write_embedded(_dst: &Path) -> Result<(), String> {
    Err("exe 内没有内嵌 DLL（用 --ours-dll 指定文件，或用带内嵌的构建）".to_string())
}

/// 自研 DLL 的来源：`--ours-dll` > 启动器旁 sidecar（若存在）> 内嵌副本。
pub fn ours_source(override_path: Option<&Path>) -> OursSource {
    if let Some(p) = override_path {
        return OursSource::File(p.to_path_buf());
    }
    let sidecar = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(OURS_FILE_NAME)));
    if let Some(p) = sidecar {
        if p.is_file() {
            return OursSource::File(p);
        }
    }
    if has_embedded_dll() {
        return OursSource::Embedded;
    }
    // 降级：返回默认 sidecar 路径，`place` 时报“找不到”，指引用户。
    OursSource::File(
        std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join(OURS_FILE_NAME)))
            .unwrap_or_else(|| PathBuf::from(OURS_FILE_NAME)),
    )
}

/// 一次瞬时替换的状态（`prepare` 产生，`restore`/`cleanup` 消费）。
#[derive(Debug)]
pub struct Swap {
    pub sdo_path: PathBuf,
    pub sdo_dir: PathBuf,
    /// 官方备份路径；`None` 表示原位置本来就没有官方文件可恢复。
    /// 内容相同的已有备份会被复用（不动它，`restore` 时再移回去）。
    pub backup_path: Option<PathBuf>,
    /// 原位置本来就是自研且无官方备份时，换走后用它回填，保证路径可用。
    pub refill_ours: Option<OursSource>,
    pub in_use_path: PathBuf,
    pub restored: bool,
}

/// 启动前换上自研 DLL（拿票后、起进程前调用，窗口越小越好）。
pub fn prepare(game_dir: &Path, ours_src: &OursSource) -> Result<Swap, String> {
    let sdo_path = game::login_dll_path(game_dir);
    let dir: PathBuf = sdo_path
        .parent()
        .ok_or_else(|| format!("登录 DLL 路径异常：{}", sdo_path.display()))?
        .to_path_buf();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建 {} 失败：{e}", dir.display()))?;
    // 上次残留的藏匿文件/marker（删不掉说明对应游戏还活着，留着不管；nonce 对不上也不会误命中）。
    clean_stale_in_use(&dir);
    swap_marker::remove(&dir);
    let in_use_path = dir.join(format!("{IN_USE_PREFIX}{}", std::process::id()));

    if !sdo_path.is_file() {
        // 全新安装：直接放自研，无需恢复。
        ours_src.place(&sdo_path)?;
        verify_ours(&sdo_path)?;
        return Ok(Swap {
            sdo_path,
            sdo_dir: dir.clone(),
            backup_path: None,
            refill_ours: None,
            in_use_path,
            restored: true,
        });
    }

    if game::dll_has_marker(&sdo_path) {
        // 原位置已经是自研（普通安装残留或上次 swap 没换回去）：不备份，换走后回填自研。
        // 若正被跑着的游戏占用导致刷新失败也不致命——文件本来就是可用的自研。
        if let Err(e) = ours_src.place(&sdo_path) {
            log::warn(&format!(
                "自研 DLL 刷新失败（沿用原位置版本，不影响启动）：{e}"
            ));
        }
        return Ok(Swap {
            sdo_path,
            sdo_dir: dir.clone(),
            backup_path: None,
            refill_ours: Some(ours_src.clone()),
            in_use_path,
            restored: false,
        });
    }

    // 官方文件先备份，再拷入自研。
    let (backup_path, backup_moved) = backup_official(&dir, &sdo_path)?;
    let rollback = |e: String| {
        if backup_moved {
            let _ = std::fs::rename(&backup_path, &sdo_path);
        }
        e
    };
    ours_src.place(&sdo_path).map_err(|e| {
        rollback(format!("{e}（已回滚官方备份）"))
    })?;
    verify_ours(&sdo_path).map_err(rollback)?;
    Ok(Swap {
        sdo_path,
        sdo_dir: dir.clone(),
        backup_path: Some(backup_path),
        refill_ours: None,
        in_use_path,
        restored: false,
    })
}

/// 备份官方文件：无备份则移过去；已有内容相同的备份则复用（不动它）；内容不同则另建带 pid
/// 后缀的备份，永不覆盖用户数据。
fn backup_official(dir: &Path, sdo_path: &Path) -> Result<(PathBuf, bool), String> {
    let first = dir.join(OFFICIAL_BAK_NAME);
    if !first.is_file() {
        std::fs::rename(sdo_path, &first).map_err(|e| {
            format!(
                "备份官方 DLL（{} → {}）失败：{e}",
                sdo_path.display(),
                first.display()
            )
        })?;
        return Ok((first, true));
    }
    if file_contents_equal(&first, sdo_path) {
        // 已有相同备份：复用，restore 时再移回去。
        return Ok((first, false));
    }
    // 官方变了（游戏更新过）：另建备份，并清掉与主备份相同的过期后缀备份。
    clean_duplicate_backups(dir, &first);
    let suffixed = dir.join(format!("{OFFICIAL_BAK_NAME}.{}", std::process::id()));
    std::fs::rename(sdo_path, &suffixed).map_err(|e| {
        format!(
            "备份官方 DLL（{} → {}）失败：{e}",
            sdo_path.display(),
            suffixed.display()
        )
    })?;
    Ok((suffixed, true))
}

fn file_contents_equal(a: &Path, b: &Path) -> bool {
    match (std::fs::read(a), std::fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// 删掉内容与主备份相同的带后缀旧备份（不同内容的留着，那是别的版本的官方）。
fn clean_duplicate_backups(dir: &Path, main_bak: &Path) {
    let prefix = format!("{OFFICIAL_BAK_NAME}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && file_contents_equal(&e.path(), main_bak) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

fn verify_ours(sdo_path: &Path) -> Result<(), String> {
    if !game::dll_has_marker(sdo_path) {
        return Err(format!(
            "在 {} 里没找到自研构建标记（来源可能不是本项目产物），停下",
            sdo_path.display()
        ));
    }
    Ok(())
}

/// 等待游戏加载登录 DLL（marker 命中或模块表可见任一即算成功）。
///
/// `nonce` 为传给游戏的 `SDO_FFXIV_SWAP` 值；`is_alive` 由调用方提供
/// （游戏提前退出就不用等满 60s）。
pub fn wait_game_loaded(
    pid: u32,
    sdo_dir: &Path,
    nonce: &str,
    mut is_alive: impl FnMut() -> bool,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if swap_marker::nonce_matches(sdo_dir, nonce) || module_loaded(pid, DLL_NAME) {
            return Ok(());
        }
        if !is_alive() {
            return Err(format!(
                "游戏进程已退出，{WAIT_TIMEOUT:?} 内未观察到 {DLL_NAME} 加载"
            ));
        }
        if start.elapsed() >= WAIT_TIMEOUT {
            return Err(format!(
                "{WAIT_TIMEOUT:?} 内未确认游戏加载 {DLL_NAME}（DLL 过旧不支持握手也会如此），官方文件未换回（盘面留自研，不影响本次游戏）"
            ));
        }
        std::thread::sleep(WAIT_POLL);
    }
}

/// 游戏加载完后换回官方（`wait_game_loaded` 成功后调用）。
pub fn restore(swap: &mut Swap) -> Result<(), String> {
    if swap.restored {
        return Ok(());
    }
    // 已加载的自研 DLL 改名藏起（Windows 上被 loader 锁住也能改名，游戏靠句柄继续跑；
    // Linux 上删映射文件本就允许，更无妨）。
    std::fs::rename(&swap.sdo_path, &swap.in_use_path).map_err(|e| {
        format!(
            "藏匿已加载 DLL（{} → {}）失败：{e}",
            swap.sdo_path.display(),
            swap.in_use_path.display()
        )
    })?;
    if let Some(bak) = swap.backup_path.clone() {
        // 官方放回去（备份是普通文件，不会被锁）。
        std::fs::rename(&bak, &swap.sdo_path).map_err(|e| {
            format!(
                "恢复官方 DLL（{} → {}）失败：{e}，自研 DLL 在 {}",
                bak.display(),
                swap.sdo_path.display(),
                swap.in_use_path.display()
            )
        })?;
        if game::dll_has_marker(&swap.sdo_path) {
            log::warn("换回去的文件仍含自研标记，请检查官方备份是否被污染");
        }
    } else if let Some(ours) = swap.refill_ours.clone() {
        // 无官方可恢复：回填自研，保证路径可用。
        ours.place(&swap.sdo_path).map_err(|e| {
            format!("回填自研 DLL 到 {} 失败：{e}", swap.sdo_path.display())
        })?;
    }
    swap_marker::remove(&swap.sdo_dir);
    swap.restored = true;
    Ok(())
}

/// 游戏退出后清理藏匿文件与 marker（删不掉就留给下次 `prepare`，不报错）。
pub fn cleanup(swap: &Swap) {
    swap_marker::remove(&swap.sdo_dir);
    if swap.in_use_path.is_file() {
        if let Err(e) = std::fs::remove_file(&swap.in_use_path) {
            log::warn(&format!(
                "清理 {} 失败（下次启动会自动再清）：{e}",
                swap.in_use_path.display()
            ));
        }
    }
}

/// 删除本目录下历次残留的藏匿文件（还被占用删不掉就跳过）。
fn clean_stale_in_use(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(IN_USE_PREFIX) {
            continue;
        }
        if let Err(err) = std::fs::remove_file(e.path()) {
            log::warn(&format!(
                "清理残留 {} 失败（可能对应游戏还活着）：{err}",
                e.path().display()
            ));
        }
    }
}

#[cfg(windows)]
fn module_loaded(pid: u32, want: &str) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W,
        TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
    };

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut me: MODULEENTRY32W = std::mem::zeroed();
        me.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Module32FirstW(snap, &mut me) != 0;
        while ok {
            let len = me
                .szModule
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(me.szModule.len());
            let name = String::from_utf16_lossy(&me.szModule[..len]);
            if name.eq_ignore_ascii_case(want) {
                found = true;
                break;
            }
            ok = Module32NextW(snap, &mut me) != 0;
        }
        CloseHandle(snap);
        found
    }
}

/// 非 Windows（Linux/wine）：模块表不可达，只靠 marker 文件。
#[cfg(not(windows))]
fn module_loaded(_pid: u32, _want: &str) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use proto::consts::DLL_BUILD_MARKER;

    fn setup(names: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "xivtl-swap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        let sdo = dir.join("sdo").join("sdologin");
        std::fs::create_dir_all(&sdo).unwrap();
        let game = dir.join("game");
        std::fs::create_dir_all(&game).unwrap();
        for (name, content) in names {
            std::fs::write(sdo.join(name), content).unwrap();
        }
        dir
    }

    fn ours_file(dir: &Path) -> OursSource {
        let p = dir.join("ours.dll");
        std::fs::write(&p, format!("junk{DLL_BUILD_MARKER}junk")).unwrap();
        OursSource::File(p)
    }

    #[test]
    fn prepare_restore_roundtrip_with_official() {
        let root = setup(&[(DLL_NAME, "official-ish bytes")]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);

        let mut sw = prepare(&game_dir, &ours).unwrap();
        // 官方已备份，自研已就位
        let bak = sw.backup_path.clone().unwrap();
        assert!(bak.is_file());
        assert!(game::dll_has_marker(&sw.sdo_path));
        assert!(!sw.restored);

        restore(&mut sw).unwrap();
        assert!(sw.restored);
        // 官方回来了，藏匿的是自研
        let sdo_bytes = std::fs::read(&sw.sdo_path).unwrap();
        assert_eq!(sdo_bytes, b"official-ish bytes");
        assert!(game::dll_has_marker(&sw.in_use_path));
        assert!(!bak.exists(), "备份应已移回原位");

        cleanup(&sw);
        assert!(!sw.in_use_path.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_reuses_identical_backup() {
        let root = setup(&[(DLL_NAME, "official-ish bytes")]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);
        // 预置与官方内容相同的备份（install.sh 留下的常见形态）
        let sdo_dir = game::login_dll_path(&game_dir)
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::write(sdo_dir.join(OFFICIAL_BAK_NAME), b"official-ish bytes").unwrap();

        let mut sw = prepare(&game_dir, &ours).unwrap();
        // 复用已有备份：原位置被换成自研，备份仍在原位（没被移走）
        assert_eq!(
            sw.backup_path.as_ref().unwrap(),
            &sdo_dir.join(OFFICIAL_BAK_NAME)
        );
        assert_eq!(
            std::fs::read(sdo_dir.join(OFFICIAL_BAK_NAME)).unwrap(),
            b"official-ish bytes"
        );
        assert!(game::dll_has_marker(&sw.sdo_path));

        restore(&mut sw).unwrap();
        assert_eq!(std::fs::read(&sw.sdo_path).unwrap(), b"official-ish bytes");
        cleanup(&sw);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_suffixes_backup_when_official_changed() {
        let root = setup(&[(DLL_NAME, "official-v2 bytes")]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);
        let sdo_dir = game::login_dll_path(&game_dir)
            .parent()
            .unwrap()
            .to_path_buf();
        // 主备份是旧版本官方
        std::fs::write(sdo_dir.join(OFFICIAL_BAK_NAME), b"official-v1 bytes").unwrap();

        let mut sw = prepare(&game_dir, &ours).unwrap();
        let bak = sw.backup_path.clone().unwrap();
        assert_ne!(bak, sdo_dir.join(OFFICIAL_BAK_NAME));
        // 主备份原样保留
        assert_eq!(
            std::fs::read(sdo_dir.join(OFFICIAL_BAK_NAME)).unwrap(),
            b"official-v1 bytes"
        );

        restore(&mut sw).unwrap();
        assert_eq!(std::fs::read(&sw.sdo_path).unwrap(), b"official-v2 bytes");
        cleanup(&sw);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_when_already_ours_refills() {
        let root = setup(&[]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);
        // 原位置已是自研且无官方备份
        let OursSource::File(p) = &ours else {
            unreachable!()
        };
        std::fs::copy(p, game::login_dll_path(&game_dir)).unwrap();

        let mut sw = prepare(&game_dir, &ours).unwrap();
        assert!(sw.backup_path.is_none());
        assert!(sw.refill_ours.is_some());

        restore(&mut sw).unwrap();
        // 回填后路径仍是可用的自研
        assert!(game::dll_has_marker(&sw.sdo_path));
        cleanup(&sw);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_missing_sdo_file_is_noop_restore() {
        let root = setup(&[]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);

        let mut sw = prepare(&game_dir, &ours).unwrap();
        assert!(sw.backup_path.is_none());
        assert!(sw.restored); // 无需恢复
        assert!(restore(&mut sw).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn prepare_rejects_missing_source() {
        let root = setup(&[(DLL_NAME, "official-ish bytes")]);
        let err = prepare(
            &root.join("game"),
            &OursSource::File(root.join("no-such-ours.dll")),
        )
        .unwrap_err();
        assert!(err.contains("--ours-dll"), "{err}");
        // 回滚：官方原样还在
        assert_eq!(
            std::fs::read(game::login_dll_path(&root.join("game"))).unwrap(),
            b"official-ish bytes"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_in_use_files_are_cleaned() {
        let root = setup(&[(DLL_NAME, "official-ish bytes")]);
        let game_dir = root.join("game");
        let ours = ours_file(&root);
        let sdo_dir = game::login_dll_path(&game_dir)
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::write(sdo_dir.join(format!("{IN_USE_PREFIX}9999")), b"x").unwrap();
        std::fs::write(sdo_dir.join("keep.txt"), b"x").unwrap();

        let _ = prepare(&game_dir, &ours).unwrap();
        assert!(!sdo_dir.join(format!("{IN_USE_PREFIX}9999")).exists());
        assert!(sdo_dir.join("keep.txt").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn ours_source_prefers_explicit_then_sidecar() {
        let custom = PathBuf::from("C:\\x\\y.dll");
        assert!(matches!(
            ours_source(Some(&custom)),
            OursSource::File(p) if p == custom
        ));
        // 无 sidecar、无显式指定时退到内嵌或默认 sidecar 路径（place 时才报错）
        let src = ours_source(None);
        match src {
            OursSource::Embedded => assert!(has_embedded_dll()),
            OursSource::File(p) => assert!(p.to_string_lossy().ends_with(OURS_FILE_NAME)),
        }
    }

    /// 内嵌副本若存在，必须含构建标记（build.rs 已断言，这里双保险）。
    #[test]
    fn embedded_bytes_contain_marker_when_present() {
        #[cfg(embedded_dll)]
        {
            assert!(EMBEDDED_DLL_BYTES
                .windows(DLL_BUILD_MARKER.len())
                .any(|w| w == DLL_BUILD_MARKER.as_bytes()));
        }
    }

    /// marker 命中即算加载成功（不依赖模块表，Linux 路径）。
    #[test]
    fn marker_match_counts_as_loaded() {
        let root = setup(&[(DLL_NAME, "official-ish bytes")]);
        let game_dir = root.join("game");
        let sdo_dir = game::login_dll_path(&game_dir)
            .parent()
            .unwrap()
            .to_path_buf();
        swap_marker::write_nonce(&sdo_dir, "NONCE-1").unwrap();
        // 游戏“没退”，marker 对上 → 立刻成功
        assert!(wait_game_loaded(1, &sdo_dir, "NONCE-1", || true).is_ok());
        // nonce 对不上且进程已死 → 报错而非死等
        assert!(wait_game_loaded(1, &sdo_dir, "NONCE-2", || false).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 本进程一定加载了 kernel32；不存在的模块名一定找不到。
    #[cfg(windows)]
    #[test]
    fn module_snapshot_finds_self_modules() {
        assert!(module_loaded(std::process::id(), "kernel32.dll"));
        assert!(!module_loaded(
            std::process::id(),
            "this-module-does-not-exist-9f3a.dll"
        ));
        assert!(!module_loaded(0xFFFFFFFF, "kernel32.dll"));
    }
}
