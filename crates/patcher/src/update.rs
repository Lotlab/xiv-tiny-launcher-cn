//! 增量更新编排：多跳 delta → 链尾补齐 → 写本地版本元数据。
//!
//! 顺序很重要：
//!
//! 1. 读本地版本（`game/LocalVersion3.xml`，回退 `ffxivgame.ver`）拿到 internal 版本；
//! 2. 拉 `ver2.dat` 得到目标 internal，沿 `from → to` 链逐跳打 delta；
//! 3. delta 只覆盖「已存在的变更文件」；没打成功的（新增文件 / delta 缺失 /
//!    源不符）记下来，链尾按目标清单整文件补下；
//! 4. **最后**才写 `game/LocalVersion3.xml`（加密）作为提交点——
//!    中途失败时本地仍是旧版本，重跑可续。
//!
//! **增量不做全量内容校验**（那要读整份安装，实测 ~118 GB）：链尾只补
//! 「补丁声明要改但没落地」和「目标清单里有、本地没有」的文件，外加版本文件。
//! 全量校验 / 修复请走 [`crate::full_download_with`]（`--force-full`）。

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::cdn::{Cdn, CdnError};
use crate::delta::{self, DeltaOutcome};
use crate::download::{Downloader, FileOutcome, Progress};
use crate::patch::{self, PatchFileList};
use crate::relpath;
use crate::version;
use crate::version_meta::{self, VersionMeta};

/// 落盘是密文的、MD5 与清单里的明文对不上，故不参与清单校验。
use crate::download::is_encrypted_file;

/// 版本文件：只有 20 字节，链尾**总是**按目标清单校验一次（`read_local` 的回退值）。
const VER_REL: &str = "game/ffxivgame.ver";

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IncrementalReport {
    /// 走了几跳 delta。
    pub hops: usize,
    pub deltas_applied: usize,
    pub deltas_skipped: usize,
    /// 链尾补齐（下载）的文件数。
    pub repaired: usize,
}

#[derive(Debug)]
pub enum UpdateError {
    /// 本地没有版本信息（全新安装应走 `full_download`）。
    NoLocal,
    /// 本地版本缺 internal（只有 `ffxivgame.ver`）。
    NoInternal(String),
    Cdn(CdnError),
    Patch(patch::PatchError),
    Json(String),
    Io(String),
    /// 磁盘空间不够。
    NoSpace { need: u64, free: u64 },
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpdateError::NoLocal => write!(f, "本地没有版本信息（全新安装请用全量下载）"),
            UpdateError::NoInternal(v) => {
                write!(f, "本地版本 {v} 只有 display、没有 internal，无法选择补丁链")
            }
            UpdateError::Cdn(e) => write!(f, "{e}"),
            UpdateError::Patch(e) => write!(f, "{e}"),
            UpdateError::Json(e) => write!(f, "补丁清单解析失败：{e}"),
            UpdateError::Io(e) => write!(f, "{e}"),
            UpdateError::NoSpace { need, free } => write!(
                f,
                "磁盘空间不足：需要 {}，可用 {}",
                crate::disk::human(*need),
                crate::disk::human(*free)
            ),
        }
    }
}

impl std::error::Error for UpdateError {}

impl From<CdnError> for UpdateError {
    fn from(e: CdnError) -> Self {
        UpdateError::Cdn(e)
    }
}
impl From<crate::cdn::ver2::Ver2Error> for UpdateError {
    fn from(e: crate::cdn::ver2::Ver2Error) -> Self {
        UpdateError::Json(e.to_string())
    }
}
impl From<patch::PatchError> for UpdateError {
    fn from(e: patch::PatchError) -> Self {
        UpdateError::Patch(e)
    }
}

/// 跑一次增量更新。`root` 是安装根（含 `game/`）。
pub fn run_incremental(
    root: &Path,
    cdn: &Cdn,
    game_id: &str,
    build_id: &str,
    max_hops: usize,
    progress: &mut dyn Progress,
) -> Result<IncrementalReport, UpdateError> {
    let local = version::read_local(root).map_err(UpdateError::Io)?.ok_or(UpdateError::NoLocal)?;
    let local_internal = local
        .internal
        .clone()
        .ok_or_else(|| UpdateError::NoInternal(local.display.clone()))?;

    let ver2 = cdn.fetch_ver2(game_id, build_id)?;
    let remote = ver2.latest()?;

    if local_internal == remote.internal {
        progress.note(&format!("已是最新（{}）", remote.display));
        return Ok(IncrementalReport::default());
    }

    let chain = patch::patch_chain(&ver2, &local_internal, &local.display, &remote.internal, max_hops)?;
    progress.note(&format!(
        "增量更新：{} → {}（{} 跳）",
        local_internal,
        remote.internal,
        chain.len()
    ));

    let auth = cdn.fetch_auth(game_id)?;
    let mut dl = Downloader::new(cdn, auth, game_id, 3).with_backup_host(ver2.backup_host());

    let work = root.join("_update");
    let zip_dir = work.join("zips");
    let delta_dir = work.join("deltas");
    let game_dir = root.join("game");

    let mut report = IncrementalReport {
        hops: chain.len(),
        ..Default::default()
    };
    // 补丁声明过但没落地的文件（新增 / delta 缺失 / 源不符），链尾统一补。
    let mut failed: Vec<String> = Vec::new();

    for (i, hop) in chain.iter().enumerate() {
        progress.note(&format!(
            "[{}/{}] {} → {}（{}）",
            i + 1,
            chain.len(),
            hop.from_display,
            hop.to_display,
            hop.to
        ));

        // 1) 补丁清单
        let full_url = format!("{}{}", hop.base_url, hop.file_list_url);
        let bytes = dl.fetch_authed_bytes(&full_url, progress)?;
        let pfl: PatchFileList =
            serde_json::from_slice(&bytes).map_err(|e| UpdateError::Json(e.to_string()))?;
        progress.note(&format!("  补丁包 {} 个", pfl.files.len()));

        // 2) 下载 zip（先看空间够不够；zip 会留到启动成功才清）
        let missing: u64 = pfl
            .files
            .iter()
            .filter(|z| match PatchFileList::zip_name(z) {
                Some(name) => {
                    !crate::cdn::verify_file(&zip_dir.join(name), z.size, &z.md5)
                }
                None => true, // 路径非法，稍后会直接报错
            })
            .map(|z| z.size)
            .sum();
        if missing > 0 {
            let need = missing + crate::disk::SPACE_MARGIN;
            crate::disk::check_free_space(root, need)
                .map_err(|(need, free)| UpdateError::NoSpace { need, free })?;
        }

        // 从这一刻起会改动本地文件：之后的错误必须中止（调用方按此判断）。
        progress.update_started();
        let mut zip_paths = Vec::new();
        for zip in &pfl.files {
            let name = PatchFileList::zip_name(zip)
                .ok_or_else(|| UpdateError::Json(format!("补丁包路径非法：{}", zip.url)))?;
            let dest = zip_dir.join(&name);
            let url = pfl.zip_url(zip);
            dl.fetch_url(&url, &dest, zip.size, &zip.md5, &format!("patch {name}"), progress)?;
            zip_paths.push(dest);
        }

        // 3) 解出 delta 并逐条应用（原地）
        let entries = patch::extract_deltas(&zip_paths, &delta_dir).map_err(UpdateError::Io)?;
        progress.note(&format!("  delta {} 条", entries.len()));
        // 全部 zip 都命中缓存时这里才是第一处改动。
        progress.update_started();
        for e in &entries {
            let game_file = game_dir.join(&e.rel_path);
            if !game_file.is_file() {
                // 新增文件：delta 打不了，留给链尾补齐。
                failed.push(e.rel_path.clone());
                continue;
            }
            let delta_file = delta_dir.join(e.delta_rel());
            match delta::apply_in_place(
                &game_file,
                &delta_file,
                &e.delta_md5,
                &e.origin_md5,
                &e.result_md5,
            ) {
                Ok(DeltaOutcome::Applied) => report.deltas_applied += 1,
                Ok(DeltaOutcome::Skipped) => report.deltas_skipped += 1,
                Err(err) => {
                    // delta 缺失/损坏、源不符：留给链尾补齐。
                    proto::log::debug(&format!("{}：{err}", e.rel_path));
                    failed.push(e.rel_path.clone());
                }
            }
        }
        // 本跳的 delta 用完即删（zip 留着，重跑可复用）。
        let _ = std::fs::remove_dir_all(&delta_dir);
    }

    // 4) 链尾：按目标清单补下「没打成功」与「本地缺失」的文件。
    //    增量不做全量校验，所以这里只查存在性（外加版本文件的小文件 MD5）。
    let list = cdn.fetch_file_list(game_id, build_id)?;
    progress.note("补齐补丁未覆盖的文件…");
    let by_path: HashMap<String, &crate::cdn::filelist::FileEntry> = list
        .files
        .iter()
        .map(|e| (relpath::canon_path(&e.path), e))
        .collect();

    let mut repair: Vec<&crate::cdn::filelist::FileEntry> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let ver_key = relpath::canon_path(VER_REL);

    // (a) 补丁声明过但没落地的（补丁里的路径相对 `game/`，清单里带 `game/` 前缀）。
    for rel in &failed {
        let key = relpath::canon_path(&format!("game/{rel}"));
        let Some(entry) = by_path.get(&key) else {
            progress.note(&format!("补丁里的 {rel} 不在目标清单里，跳过"));
            continue;
        };
        if seen.insert(key) {
            repair.push(entry);
        }
    }
    // (b) 目标清单里有、本地没有的（新增文件 / 被删掉的）；(c) 版本文件。
    for entry in &list.files {
        if is_encrypted_file(&entry.path) {
            continue; // 最后单独写加密元数据
        }
        let key = relpath::canon_path(&entry.path);
        if (key == ver_key || !list.local_path(root, entry).exists()) && seen.insert(key) {
            repair.push(entry);
        }
    }

    if !repair.is_empty() {
        let need: u64 = repair.iter().map(|e| e.size).sum();
        crate::disk::check_free_space(root, need + crate::disk::SPACE_MARGIN)
            .map_err(|(need, free)| UpdateError::NoSpace { need, free })?;
    }
    progress.update_started();
    for entry in repair {
        let dest = list.local_path(root, entry);
        if dl.fetch_one(&list, entry, &dest, progress)? == FileOutcome::Downloaded {
            report.repaired += 1;
        }
    }

    // 5) 提交点：写加密版本元数据（`ffxivgame.ver` 已在链尾按清单校验/补下）
    write_meta(root, game_id, build_id, &remote, progress)?;

    // `_update/` 留给启动成功后清理（保留 zip 便于中断重跑）。

    progress.note(&format!(
        "更新完成：delta {} 条（跳过 {}），补齐 {} 个文件",
        report.deltas_applied, report.deltas_skipped, report.repaired
    ));
    Ok(report)
}
/// 写 `game/LocalVersion3.xml`（加密，原子写）。
pub fn write_meta(
    root: &Path,
    game_id: &str,
    build_id: &str,
    remote: &crate::cdn::ver2::RemoteVersion,
    progress: &mut dyn Progress,
) -> Result<(), UpdateError> {
    let game = root.join("game");
    std::fs::create_dir_all(&game).map_err(|e| UpdateError::Io(e.to_string()))?;

    let meta = VersionMeta::new(game_id, build_id, &remote.internal, &remote.view_full());
    let encoded = version_meta::build(&meta);
    let path = game.join("LocalVersion3.xml");
    proto::paths::write_atomic(&path, &encoded)
        .map_err(|e| UpdateError::Io(format!("写 LocalVersion3.xml 失败：{e}")))?;

    progress.note(&format!("已写入本地版本 {}（{}）", remote.display, remote.internal));
    Ok(())
}
