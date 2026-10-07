//! `patcher` — FFXIV CN 游戏本体版本检查与增量更新。
//!
//! 职责边界：
//!
//! - **网络**：CDN 版本检查、`v3ctrl.xml` 鉴权、文件清单、patch 包下载。
//! - **落盘**：游戏文件下载/校验/断点续传、差分包解压与应用、本地版本元数据。
//! - **不做**：登录协议（`sdo-client`）、UI 渲染与起进程（`launcher`）。
//!
//! `game_id` / `build_id` 由调用方传入（启动器用 `sdo_client::GAME_APP_ID` /
//! `sdo_client::BUILD_ID`）；本 crate 不依赖 `sdo-client`。
//!
//! 密钥（CDN RSA 公钥、本地元数据 3DES 密钥）由 `build.rs` 打包时注入，见 [`keys`]。

pub mod cdn;
pub mod crypto;
pub mod delta;
pub mod disk;
pub mod download;
pub mod hash;
pub mod keys;
pub mod patch;
pub mod relpath;
pub mod update;
pub mod version;
pub mod version_meta;

use std::path::Path;

pub use version::{LocalVersion, LocalVersionSource, UpdateDecision};
pub use version_meta::{VersionEntry, VersionMeta};

/// 版本检查结果。
#[derive(Debug, Clone)]
pub struct CheckOutcome {
    /// CDN 当前最新版本。
    pub remote: cdn::ver2::RemoteVersion,
    /// 本地版本；`None` = 全新安装。
    pub local: Option<LocalVersion>,
    pub decision: UpdateDecision,
    /// 同一次 `ver2.dat` 请求的解析结果，供 [`CheckOutcome::plan_incremental_chain`]。
    ver2: cdn::ver2::Ver2,
}

impl CheckOutcome {
    /// 预演增量链（本地 internal → CDN 目标），不触网。
    ///
    /// 链走不通（本地版本过旧 / CDN 无对应包）时返回错误；调用方据此
    /// **在下载之前**跳过更新，而不是把登录拦下。返回空链表示 internal 已是最新。
    pub fn plan_incremental_chain(
        &self,
        max_hops: usize,
    ) -> Result<Vec<patch::PatchInfo>, update::UpdateError> {
        let local = self.local.as_ref().ok_or(update::UpdateError::NoLocal)?;
        let internal = local
            .internal
            .clone()
            .ok_or_else(|| update::UpdateError::NoInternal(local.display.clone()))?;
        patch::patch_chain(
            &self.ver2,
            &internal,
            &local.display,
            &self.remote.internal,
            max_hops,
        )
        .map_err(update::UpdateError::Patch)
    }
}

#[derive(Debug)]
pub enum CheckError {
    Cdn(cdn::CdnError),
    Ver2(cdn::ver2::Ver2Error),
    Local(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CheckError::Cdn(e) => write!(f, "{e}"),
            CheckError::Ver2(e) => write!(f, "{e}"),
            CheckError::Local(e) => write!(f, "读取本地版本失败：{e}"),
        }
    }
}

impl std::error::Error for CheckError {}

impl From<cdn::CdnError> for CheckError {
    fn from(e: cdn::CdnError) -> Self {
        CheckError::Cdn(e)
    }
}

impl From<cdn::ver2::Ver2Error> for CheckError {
    fn from(e: cdn::ver2::Ver2Error) -> Self {
        CheckError::Ver2(e)
    }
}

/// 增量更新：多跳 delta + 链尾补齐 + 写本地版本。
///
/// 需要自带 CDN 客户端（证书 / 代理策略由调用方决定）。
pub fn incremental_update_with(
    root: &Path,
    cdn: &cdn::Cdn,
    game_id: &str,
    build_id: &str,
    max_hops: usize,
    progress: &mut dyn download::Progress,
) -> Result<update::IncrementalReport, update::UpdateError> {
    update::run_incremental(root, cdn, game_id, build_id, max_hops, progress)
}

/// 全量校验计划 + 这次校验用的清单与远端版本（只读本地，不落盘、不下载）。
///
/// 「先校验、再问是否下载」：全量校验要把整份安装读一遍（实测 ~118 GB），
/// 用户该先看到缺口有多大，再决定要不要下载（见 `docs/UPDATE.md`「更新确认」）。
#[derive(Debug)]
pub struct FullPlan {
    /// 校验结果：清单总数 / 通过数 / 待下载数与字节数。
    pub plan: download::Plan,
    /// 这次校验用的文件清单。
    list: cdn::filelist::FileList,
    /// CDN 当前最新版本（执行计划时用它写本地元数据）。
    remote: cdn::ver2::RemoteVersion,
}

impl FullPlan {
    /// CDN 当前最新版本。
    pub fn remote(&self) -> &cdn::ver2::RemoteVersion {
        &self.remote
    }
}

/// 全量校验（**不下载**）：拉清单并逐个比对本地文件，返回待下载计划。
///
/// 调用方拿到 [`FullPlan`] 后应该先汇总给用户，确认了再走 [`run_full_plan_with`]。
pub fn verify_full_with(
    root: &Path,
    cdn: &cdn::Cdn,
    game_id: &str,
    build_id: &str,
    progress: &mut dyn download::Progress,
) -> Result<FullPlan, CheckError> {
    let ver2 = cdn.fetch_ver2(game_id, build_id)?;
    let remote = ver2.latest()?;
    let list = cdn.fetch_file_list(game_id, build_id)?;
    let plan = download::plan(root, &list, progress);
    Ok(FullPlan {
        plan,
        list,
        remote,
    })
}

/// 执行全量校验计划：只下载 [`FullPlan::plan`] 里待补齐的文件，最后写加密版本元数据（提交点）。
///
/// 计划为空（本地与清单一字不差）时不取鉴权、不碰网络，直接提交版本元数据。
pub fn run_full_plan_with(
    root: &Path,
    cdn: &cdn::Cdn,
    game_id: &str,
    build_id: &str,
    full: FullPlan,
    progress: &mut dyn download::Progress,
) -> Result<download::Report, CheckError> {
    let report = if full.plan.is_empty() {
        download::Report {
            total: full.plan.total,
            skipped: full.plan.skipped,
            ..Default::default()
        }
    } else {
        let auth = cdn.fetch_auth(game_id)?;
        let mut dl = download::Downloader::new(cdn, auth, game_id, 3);
        dl.run_plan(root, &full.list, &full.plan, progress)
            .map_err(CheckError::Cdn)?
    };
    // 提交点：写加密版本元数据（`ffxivgame.ver` 属于游戏文件，已随清单下载）。
    update::write_meta(root, game_id, build_id, &full.remote, progress)
        .map_err(|e| CheckError::Local(e.to_string()))?;
    Ok(report)
}

/// 全量安装 / 修复：拉文件清单，遍历校验并下载（校验 + 下载一体）。
///
/// 等价于 [`verify_full_with`] 后接 [`run_full_plan_with`]；需要「先校验、再问
/// 是否下载」的调用方请分开调那两个。
pub fn full_download_with(
    root: &Path,
    cdn: &cdn::Cdn,
    game_id: &str,
    build_id: &str,
    progress: &mut dyn download::Progress,
) -> Result<download::Report, CheckError> {
    let full = verify_full_with(root, cdn, game_id, build_id, progress)?;
    run_full_plan_with(root, cdn, game_id, build_id, full, progress)
}

/// 版本检查：拉 CDN `ver2.dat`，与 `root/game` 下的本地版本比较。
///
/// 需要自带 CDN 客户端（便于测试 / `--insecure-cdn`）。
pub fn check_update_with(
    root: &Path,
    cdn: &cdn::Cdn,
    game_id: &str,
    build_id: &str,
) -> Result<CheckOutcome, CheckError> {
    let ver2 = cdn.fetch_ver2(game_id, build_id)?;
    let remote = ver2.latest()?;
    let local = version::read_local(root).map_err(CheckError::Local)?;
    let decision = version::decide(&remote.display, local.as_ref().map(|l| l.display.as_str()));
    Ok(CheckOutcome {
        remote,
        local,
        decision,
        ver2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(internal: Option<&str>, display: &str) -> CheckOutcome {
        let ver2: cdn::ver2::Ver2 = serde_json::from_str(
            r#"{
              "baseUrl": "https://h/diff",
              "areas": [{"max": "0.0.0.29"}],
              "packages": [
                {"from":"0.0.0.27","to":"0.0.0.29",
                 "versionView":"2026.09.15.0000.0000_7.56","fileListUrl":"/p.dat"}
              ]
            }"#,
        )
        .unwrap();
        let remote = ver2.latest().unwrap();
        CheckOutcome {
            remote,
            local: Some(LocalVersion {
                display: display.into(),
                internal: internal.map(str::to_string),
                source: match internal {
                    Some(_) => LocalVersionSource::Meta,
                    None => LocalVersionSource::VerFile,
                },
            }),
            decision: UpdateDecision::UpdateAvailable,
            ver2,
        }
    }

    /// 预演链：可走则给出跳数，已最新给出空链，走不通 / 缺 internal 报错。
    #[test]
    fn plan_incremental_chain_covers_all_cases() {
        assert_eq!(
            outcome(Some("0.0.0.27"), "2026.09.01.0000.0000")
                .plan_incremental_chain(32)
                .unwrap()
                .len(),
            1
        );
        assert!(outcome(Some("0.0.0.29"), "2026.09.15.0000.0000")
            .plan_incremental_chain(32)
            .unwrap()
            .is_empty());
        // 本地过旧：CDN 链里没有 0.0.0.13 这一跳。
        assert!(outcome(Some("0.0.0.13"), "x").plan_incremental_chain(32).is_err());
        // 只有 ffxivgame.ver，没有 internal。
        assert!(outcome(None, "x").plan_incremental_chain(32).is_err());
    }
}
