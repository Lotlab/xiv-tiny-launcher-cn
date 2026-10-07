//! 全量下载 / 修复：遍历文件清单，跳过已校验通过的，下载其余。
//!
//! 校验与下载是分开的两步：[`plan`] 只比对本地文件（读整份安装），返回待下载
//! 计划；[`Downloader::run_plan`] 才落盘。调用方据此**先校验、再问用户是否下载**
//! （见 `docs/UPDATE.md`「更新确认」）；[`Downloader::run`] 是两步合一的便捷写法。
//!
//! 差分包（M3）会复用这里的 [`Downloader`] 下载 patch zip。

use std::path::Path;

use crate::cdn::{auth, filelist, Cdn, CdnError};

/// 下载进度回调。
pub trait Progress {
    /// 开始，给出总文件数与总字节数。
    fn begin(&mut self, _total_files: usize, _total_bytes: u64) {}
    /// 开始全量校验，给出要检查的文件数（**不含**密文文件）。
    fn verify_begin(&mut self, _total_files: usize) {}
    /// 全量校验进度：已检查 `done` / 共 `total`，当前文件 `path`。
    ///
    /// 实现方自己限流（大文件算 MD5 很慢，每个文件都刷新会淹掉终端）。
    fn verify_progress(&mut self, _done: usize, _total: usize, _path: &str) {}
    /// 全量校验结束（收尾原地状态行；结果由调用方另行汇总打印）。
    fn verify_end(&mut self) {}
    /// 开始处理某个文件。
    fn file_start(&mut self, _path: &str, _size: u64) {}
    /// 某文件已下载字节数更新。
    fn file_progress(&mut self, _path: &str, _done: u64, _total: u64) {}
    /// 某文件结束。
    fn file_end(&mut self, _path: &str, _outcome: FileOutcome) {}
    /// 一次性提示（重试、鉴权刷新等）。
    fn note(&mut self, _msg: &str) {}
    /// 即将真正改动本地文件（下 patch / 打 delta / 补齐缺失）。
    ///
    /// 调用方据此区分「更新还没开始」与「更新已进行中」的错误：前者可以安全
    /// 跳过并继续登录，后者必须中止（见 `patcher::update` 模块头）。
    fn update_started(&mut self) {}
}

/// 单个文件的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    /// 本地已存在且 size+MD5 通过。
    Skipped,
    /// 本次下载（含续传）完成。
    Downloaded,
    /// 最终失败（错误会继续上抛；只是给进度回调一个收尾机会）。
    Failed,
}

/// 汇总。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub total: usize,
    pub skipped: usize,
    pub downloaded: usize,
    pub bytes: u64,
}

/// 全量校验计划：清单里需要下载的条目（本地缺失 / size 或 MD5 不符）。
///
/// 由 [`plan`] 产出、交给 [`Downloader::run_plan`] 执行；两者之间正好是「问用户
/// 要不要下载」的位置。
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// 实际纳入校验的文件数（**不含**落盘是密文的 `game/LocalVersion3.xml`）。
    pub total: usize,
    /// 校验通过、无需下载的文件数。
    pub skipped: usize,
    /// 待下载文件的字节数合计。
    pub bytes: u64,
    /// 待下载条目在 `list.files` 里的下标（保持清单顺序）。
    pending: Vec<usize>,
}

impl Plan {
    /// 待下载的文件数。
    pub fn pending_files(&self) -> usize {
        self.pending.len()
    }

    /// 没有需要下载的文件（整份清单一字不差）。
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 按清单顺序迭代待下载的条目。
    pub fn pending_entries<'a>(
        &'a self,
        list: &'a filelist::FileList,
    ) -> impl Iterator<Item = &'a filelist::FileEntry> {
        self.pending.iter().map(move |&i| &list.files[i])
    }
}

/// 落盘是**密文**、MD5 与清单里的明文对不上的文件：下载时跳过，
/// 由更新流程最后单独写入（见 `version_meta::build`）。
const ENCRYPTED_META_REL: &str = "game/LocalVersion3.xml";

/// 清单路径是否属于「密文文件」。
pub fn is_encrypted_file(path: &str) -> bool {
    path.replace('\\', "/").eq_ignore_ascii_case(ENCRYPTED_META_REL)
}

/// 取 URL 的 host（日志用；解析失败时原样返回）。
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// 只校验不下载：逐个比对清单条目的 size + MD5，返回待下载计划。
///
/// 全量校验要把整份安装读一遍（实测 ~118 GB，大文件算 MD5 很慢），所以调用方
/// 应该先把 `Plan` 汇总给用户看，再问是否下载（见 `docs/UPDATE.md`「更新确认」）。
pub fn plan(root: &Path, list: &filelist::FileList, progress: &mut dyn Progress) -> Plan {
    let total = list
        .files
        .iter()
        .filter(|e| !is_encrypted_file(&e.path))
        .count();
    progress.verify_begin(total);

    let mut plan = Plan {
        total,
        ..Default::default()
    };
    let mut done = 0usize;
    for (i, entry) in list.files.iter().enumerate() {
        if is_encrypted_file(&entry.path) {
            continue; // 密文文件不按明文清单校验（由更新流程最后单独写入）
        }
        done += 1;
        progress.verify_progress(done, total, &entry.path);
        if crate::cdn::verify_local(&list.local_path(root, entry), entry) {
            plan.skipped += 1;
        } else {
            plan.bytes += entry.size;
            plan.pending.push(i);
        }
    }
    progress.verify_end();
    plan
}

/// 文件下载器：持有鉴权材料，配置失效（458）时自行刷新。
///
/// 重试策略（与官方行为对齐，并覆盖多节点边缘抖动）：
/// - `458`：CDN 配置失效 → 睡 1 秒 → 刷新鉴权 → 同 URL 重试；
/// - `403`：边缘节点拒绝（鉴权本身无错）→ 重建连接（重新 DNS 解析，
///   GSLB 轮换下可能落到另一个节点）→ 按 10s / 30s / 60s 退避 → 在主备
///   host 之间轮换重试；
/// - `5xx`：同 403 处理（可能是单节点故障）；
/// - 其他错误（断线、校验失败等）：保持原有短退避（2s / 4s / 8s）重试。
pub struct Downloader<'a> {
    cdn: &'a Cdn,
    auth: auth::CdnAuth,
    game_id: String,
    retries: u32,
    backup_host: Option<String>,
}

/// 边缘拒绝（403）/ 服务端错误（5xx）的退避秒数：累计约 100 秒，
/// 覆盖分钟级的边缘抖动窗口。
const EDGE_BACKOFF_SECS: [u64; 3] = [10, 30, 60];

impl<'a> Downloader<'a> {
    pub fn new(cdn: &'a Cdn, auth: auth::CdnAuth, game_id: impl Into<String>, retries: u32) -> Self {
        Downloader {
            cdn,
            auth,
            game_id: game_id.into(),
            retries,
            backup_host: None,
        }
    }

    /// 设置备用 host（来自 `ver2.backupBaseUrl`）：边缘拒绝时换 host 重试。
    ///
    /// 鉴权 hash 只覆盖 path，换 host 后鉴权仍然有效。
    pub fn with_backup_host(mut self, host: Option<String>) -> Self {
        self.backup_host = host;
        self
    }

    pub fn auth(&self) -> &auth::CdnAuth {
        &self.auth
    }

    fn refresh_auth(&mut self) -> Result<(), CdnError> {
        self.auth = self.cdn.fetch_auth(&self.game_id)?;
        Ok(())
    }

    /// 主备候选 URL：先主后备；相同/无备用时只有主。
    fn candidates(&self, url: &str) -> Vec<String> {
        let mut out = vec![url.to_string()];
        if let Some(b) = self.backup_host.as_deref() {
            let swapped = crate::cdn::swap_host(url, b);
            if swapped != out[0] {
                out.push(swapped);
            }
        }
        out
    }

    /// 换节点：先用 HTTPDNS 拿全量边缘 IP 并 pin 住轮换，DoH 不可用时
    /// 回退到普通重建连接（重新系统 DNS 解析）。
    ///
    /// 全程静默（终端提示由调用方的人话消息负责，技术细节记 debug 日志）；
    /// 失败也不中断重试（沿用旧连接）。
    fn reconnect(&self, url: &str) {
        let host = host_of(url);
        if self.cdn.refresh_pins(&host).is_err() {
            proto::log::debug(&format!("HTTPDNS 解析 {host} 失败，重建普通连接"));
            if let Err(e) = self.cdn.reconnect() {
                proto::log::debug(&format!("重建 CDN 连接失败（沿用旧连接）：{e}"));
            }
        } else {
            proto::log::debug(&format!("HTTPDNS 已切换 {host} 的边缘节点"));
        }
    }

    /// 无 `Progress` 场景的同款换节点（`fetch_authed_bytes` 用，同样只记日志）。
    fn repin(cdn: &Cdn, url: &str) {
        let host = host_of(url);
        match cdn.refresh_pins(&host) {
            Ok(n) => proto::log::debug(&format!("HTTPDNS 解析 {host}：{n} 个边缘 IP")),
            Err(e) => {
                proto::log::debug(&format!("HTTPDNS 解析 {host} 失败（{e}），重建普通连接"));
                if let Err(e) = cdn.reconnect() {
                    proto::log::debug(&format!("重建 CDN 连接失败（沿用旧连接）：{e}"));
                }
            }
        }
    }

    /// 边缘拒绝/服务端错误时的等待秒数（按已重试次数取档）。
    fn edge_wait_secs(attempt: u32) -> u64 {
        EDGE_BACKOFF_SECS[(attempt as usize).saturating_sub(1).min(EDGE_BACKOFF_SECS.len() - 1)]
    }

    /// 带鉴权拉一个小文件（补丁清单等），458 刷新重试，403 换节点退避重试。
    ///
    /// 终端只说人话（自动重试中），技术细节记 debug 日志。
    pub fn fetch_authed_bytes(
        &mut self,
        url: &str,
        progress: &mut dyn Progress,
    ) -> Result<Vec<u8>, CdnError> {
        let urls = self.candidates(url);
        let mut attempt = 0u32;
        loop {
            let u = &urls[(attempt as usize) % urls.len()];
            match self.cdn.get_authed(&self.auth, u) {
                Ok(b) => return Ok(b),
                Err(CdnError::AuthExpired { status }) if attempt < self.retries => {
                    attempt += 1;
                    progress.note(&format!("下载配置更新，1 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!("补丁清单：CDN 配置失效 HTTP {status}，刷新鉴权后重试"));
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    self.refresh_auth()?;
                }
                Err(CdnError::EdgeRejected { status, body_len, .. })
                    if attempt < self.retries =>
                {
                    attempt += 1;
                    let wait = Self::edge_wait_secs(attempt);
                    progress.note(&format!("下载受阻，{wait} 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!(
                        "补丁清单：边缘拒绝 HTTP {status} 响应体 {body_len} 字节，换节点（{}）{wait}s 后重试",
                        host_of(&urls[(attempt as usize) % urls.len()])
                    ));
                    Self::repin(self.cdn, u);
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// 处理单个文件：已通过校验则跳过，否则下载（含续传与重试）。
    pub fn fetch_one(
        &mut self,
        list: &filelist::FileList,
        entry: &filelist::FileEntry,
        dest: &Path,
        progress: &mut dyn Progress,
    ) -> Result<FileOutcome, CdnError> {
        let url = list.url_for(entry);
        self.fetch_url(&url, dest, entry.size, &entry.hash, &entry.path, progress)
    }

    /// 下载任意 URL 到 `dest`（补丁 zip 等也走这里），含续传、校验、重试。
    ///
    /// 失败时在主备 host 间轮换、重建连接后按退避等待再试（见 [`Downloader`]）。
    pub fn fetch_url(
        &mut self,
        url: &str,
        dest: &Path,
        size: u64,
        md5: &str,
        label: &str,
        progress: &mut dyn Progress,
    ) -> Result<FileOutcome, CdnError> {
        if crate::cdn::verify_file(dest, size, md5) {
            return Ok(FileOutcome::Skipped);
        }

        progress.file_start(label, size);
        let urls = self.candidates(url);
        let mut attempt = 0u32;
        loop {
            let u = &urls[(attempt as usize) % urls.len()];
            let mut on_progress = |done: u64| progress.file_progress(label, done, size);
            match self
                .cdn
                .download_file(&self.auth, u, dest, size, md5, &mut on_progress)
            {
                Ok(_) => return Ok(FileOutcome::Downloaded),
                Err(CdnError::AuthExpired { status }) if attempt < self.retries => {
                    attempt += 1;
                    progress.note(&format!("下载配置更新，1 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!("{label}：CDN 配置失效 HTTP {status}，刷新鉴权后重试"));
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    self.refresh_auth()?;
                }
                Err(CdnError::EdgeRejected { status, body_len, .. })
                    if attempt < self.retries =>
                {
                    attempt += 1;
                    let wait = Self::edge_wait_secs(attempt);
                    progress.note(&format!("下载受阻，{wait} 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!(
                        "{label}：边缘拒绝 HTTP {status} 响应体 {body_len} 字节，换节点（{}）{wait}s 后重试",
                        host_of(&urls[(attempt as usize) % urls.len()])
                    ));
                    self.reconnect(u);
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                }
                Err(CdnError::Http { status, body_len, .. })
                    if status >= 500 && attempt < self.retries =>
                {
                    // 服务端错误也可能是单节点故障：同样换节点 + 退避。
                    attempt += 1;
                    let wait = Self::edge_wait_secs(attempt);
                    progress.note(&format!("下载受阻，{wait} 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!(
                        "{label}：CDN 服务端错误 HTTP {status} 响应体 {body_len} 字节，换节点（{}）{wait}s 后重试",
                        host_of(&urls[(attempt as usize) % urls.len()])
                    ));
                    self.reconnect(u);
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                }
                Err(e) if attempt < self.retries => {
                    attempt += 1;
                    let wait = 1u64 << attempt;
                    progress.note(&format!("下载出错，{wait} 秒后自动重试（{attempt}/{}）", self.retries));
                    proto::log::debug(&format!("{label}：{e}，{wait}s 后重试"));
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                }
                Err(e) => {
                    // 收尾状态行；错误仍照原样上抛。
                    progress.file_end(label, FileOutcome::Failed);
                    return Err(e);
                }
            }
        }
    }

    /// 按校验计划下载：只处理 [`Plan::pending_entries`]，本地已通过校验的文件不动。
    pub fn run_plan(
        &mut self,
        root: &Path,
        list: &filelist::FileList,
        plan: &Plan,
        progress: &mut dyn Progress,
    ) -> Result<Report, CdnError> {
        progress.begin(plan.pending_files(), plan.bytes);

        let mut report = Report {
            total: plan.total,
            skipped: plan.skipped,
            ..Default::default()
        };
        if !plan.is_empty() {
            // 从这一刻起会改动本地文件：调用方按 `update_started` 区分错误性质。
            progress.update_started();
        }
        for entry in plan.pending_entries(list) {
            let dest = list.local_path(root, entry);
            let outcome = self.fetch_one(list, entry, &dest, progress)?;
            match outcome {
                FileOutcome::Skipped => report.skipped += 1,
                FileOutcome::Downloaded => {
                    report.downloaded += 1;
                    report.bytes += entry.size;
                }
                // `run_plan` 里不会出现：失败会直接 `?` 上抛。
                FileOutcome::Failed => {}
            }
            progress.file_end(&entry.path, outcome);
        }
        Ok(report)
    }

    /// 遍历整份清单：先校验再下载（[`plan`] + [`Downloader::run_plan`] 的便捷写法）。
    ///
    /// 需要「先校验、再问是否下载」时请分开调用那两个。
    pub fn run(
        &mut self,
        root: &Path,
        list: &filelist::FileList,
        progress: &mut dyn Progress,
    ) -> Result<Report, CdnError> {
        let plan = plan(root, list, progress);
        self.run_plan(root, list, &plan, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 只关心结果、不要任何输出的进度实现。
    struct Silent;
    impl Progress for Silent {}

    #[test]
    fn encrypted_meta_matching() {
        assert!(is_encrypted_file("game\\LocalVersion3.xml"));
        assert!(is_encrypted_file("game/LocalVersion3.xml"));
        assert!(!is_encrypted_file("game\\ffxivgame.ver"));
    }

    #[test]
    fn plan_splits_verified_from_pending() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("game")).unwrap();
        std::fs::write(dir.path().join("game/a.dat"), b"hello").unwrap();
        // 大小一样但内容不符（MD5 才是判据），且密文文件不进校验。
        std::fs::write(dir.path().join("game/c.dat"), b"hello").unwrap();

        let entry = |path: &str, body: &[u8], size: u64| filelist::FileEntry {
            path: path.into(),
            size,
            hash: crate::hash::md5_hex_upper(body),
        };
        let list = filelist::FileList {
            hash_base_path: "https://h/base".into(),
            identifier: "1".into(),
            hash_id: "2".into(),
            files: vec![
                entry("game\\a.dat", b"hello", 5),  // 通过
                entry("game\\b.dat", b"xyz", 3),    // 缺失
                entry("game\\c.dat", b"world", 5),  // MD5 不符
                entry("game\\LocalVersion3.xml", b"x", 1), // 密文：跳过
            ],
        };

        let plan = plan(dir.path(), &list, &mut Silent);
        assert_eq!(plan.total, 3);
        assert_eq!(plan.skipped, 1);
        assert!(!plan.is_empty());
        assert_eq!(plan.pending_files(), 2);
        assert_eq!(plan.bytes, 3 + 5);
        let pending: Vec<&str> = plan
            .pending_entries(&list)
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(pending, vec!["game\\b.dat", "game\\c.dat"]);
    }

    #[test]
    fn plan_is_empty_when_everything_matches() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("game")).unwrap();
        std::fs::write(dir.path().join("game/a.dat"), b"hello").unwrap();
        let list = filelist::FileList {
            hash_base_path: "https://h/base".into(),
            identifier: "1".into(),
            hash_id: "2".into(),
            files: vec![filelist::FileEntry {
                path: "game\\a.dat".into(),
                size: 5,
                hash: crate::hash::md5_hex_upper(b"hello"),
            }],
        };
        let plan = plan(dir.path(), &list, &mut Silent);
        assert!(plan.is_empty());
        assert_eq!((plan.total, plan.skipped, plan.bytes), (1, 1, 0));
    }

    /// 进度回调的顺序契约：verify_begin → verify_progress×N → verify_end，且只报非密文文件。
    #[test]
    fn plan_reports_verify_progress() {
        #[derive(Default)]
        struct Recorder {
            began: Option<usize>,
            seen: Vec<String>,
            ended: bool,
        }
        impl Progress for Recorder {
            fn verify_begin(&mut self, total: usize) {
                self.began = Some(total);
            }
            fn verify_progress(&mut self, done: usize, total: usize, path: &str) {
                assert_eq!(done, self.seen.len() + 1);
                assert_eq!(total, 2);
                self.seen.push(path.to_string());
            }
            fn verify_end(&mut self) {
                self.ended = true;
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let list = filelist::FileList {
            hash_base_path: "https://h/base".into(),
            identifier: "1".into(),
            hash_id: "2".into(),
            files: vec![
                filelist::FileEntry {
                    path: "game\\a.dat".into(),
                    size: 1,
                    hash: crate::hash::md5_hex_upper(b"a"),
                },
                filelist::FileEntry {
                    path: "game\\LocalVersion3.xml".into(),
                    size: 1,
                    hash: crate::hash::md5_hex_upper(b"b"),
                },
                filelist::FileEntry {
                    path: "game\\b.dat".into(),
                    size: 1,
                    hash: crate::hash::md5_hex_upper(b"b"),
                },
            ],
        };

        let mut rec = Recorder::default();
        let plan = plan(dir.path(), &list, &mut rec);
        assert_eq!(rec.began, Some(2));
        assert_eq!(rec.seen, vec!["game\\a.dat", "game\\b.dat"]);
        assert!(rec.ended);
        assert_eq!(plan.pending_files(), 2); // 两个文件都不存在
    }

    fn test_downloader(backup: Option<String>) -> Downloader<'static> {
        // `Box::leak` 让 `&Cdn` 活到 `'static`，只为单测搭架子。
        let cdn: &'static Cdn =
            Box::leak(Box::new(Cdn::with_options(false, crate::cdn::ProxyMode::Env).unwrap()));
        let auth = auth::CdnAuth {
            project_md5_key: "P".into(),
            referer_value: "UA".into(),
            cdn_token: "T".into(),
            config_flag: 2,
        };
        Downloader::new(cdn, auth, "100001900", 3).with_backup_host(backup)
    }

    #[test]
    fn candidates_fall_back_to_backup_host() {
        let dl = test_downloader(Some("backup.example".into()));
        let got = dl.candidates("https://ff14.jijiagames.com/a/b.dat");
        assert_eq!(
            got,
            vec![
                "https://ff14.jijiagames.com/a/b.dat",
                "https://backup.example/a/b.dat"
            ]
        );
        // 无备用时只有主。
        let dl = test_downloader(None);
        assert_eq!(dl.candidates("https://h/x"), vec!["https://h/x"]);
        // 备用与主相同 → 去重后只有主。
        let dl = test_downloader(Some("h".into()));
        assert_eq!(dl.candidates("https://h/x"), vec!["https://h/x"]);
    }

    #[test]
    fn edge_backoff_covers_minutes() {
        // 10s → 30s → 60s，之后保持 60s。
        assert_eq!(
            (1..=5).map(Downloader::edge_wait_secs).collect::<Vec<_>>(),
            vec![10, 30, 60, 60, 60]
        );
    }

    #[test]
    fn host_of_extracts_host() {
        assert_eq!(host_of("https://a.example:8443/x"), "a.example");
        assert_eq!(host_of("not a url"), "not a url");
    }
}
