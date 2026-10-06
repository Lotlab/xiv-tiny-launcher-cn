//! 全量下载 / 修复：遍历文件清单，跳过已校验通过的，下载其余。
//!
//! 差分包（M3）会复用这里的 [`Downloader`] 下载 patch zip。

use std::path::Path;

use crate::cdn::{auth, filelist, Cdn, CdnError};

/// 下载进度回调。
pub trait Progress {
    /// 开始，给出总文件数与总字节数。
    fn begin(&mut self, _total_files: usize, _total_bytes: u64) {}
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

/// 落盘是**密文**、MD5 与清单里的明文对不上的文件：下载时跳过，
/// 由更新流程最后单独写入（见 `version_meta::build`）。
const ENCRYPTED_META_REL: &str = "game/LocalVersion3.xml";

/// 清单路径是否属于「密文文件」。
pub fn is_encrypted_file(path: &str) -> bool {
    path.replace('\\', "/").eq_ignore_ascii_case(ENCRYPTED_META_REL)
}

/// 文件下载器：持有鉴权材料，鉴权过期时自行刷新。
pub struct Downloader<'a> {
    cdn: &'a Cdn,
    auth: auth::CdnAuth,
    game_id: String,
    retries: u32,
}

impl<'a> Downloader<'a> {
    pub fn new(cdn: &'a Cdn, auth: auth::CdnAuth, game_id: impl Into<String>, retries: u32) -> Self {
        Downloader {
            cdn,
            auth,
            game_id: game_id.into(),
            retries,
        }
    }

    pub fn auth(&self) -> &auth::CdnAuth {
        &self.auth
    }

    fn refresh_auth(&mut self) -> Result<(), CdnError> {
        self.auth = self.cdn.fetch_auth(&self.game_id)?;
        Ok(())
    }

    /// 带鉴权拉一个小文件（补丁清单等），403 时刷新鉴权重试。
    pub fn fetch_authed_bytes(&mut self, url: &str) -> Result<Vec<u8>, CdnError> {
        let mut attempt = 0u32;
        loop {
            match self.cdn.get_authed(&self.auth, url) {
                Ok(b) => return Ok(b),
                Err(CdnError::AuthExpired { status }) if attempt < self.retries => {
                    attempt += 1;
                    let _ = status;
                    self.refresh_auth()?;
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

    /// 下载任意 URL 到 `dest`（补丁 zip 等也走这里），含续传、校验、鉴权刷新与重试。
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
        let mut attempt = 0u32;
        loop {
            let mut on_progress = |done: u64| progress.file_progress(label, done, size);
            match self
                .cdn
                .download_file(&self.auth, url, dest, size, md5, &mut on_progress)
            {
                Ok(_) => return Ok(FileOutcome::Downloaded),
                Err(CdnError::AuthExpired { status }) if attempt < self.retries => {
                    attempt += 1;
                    progress.note(&format!("鉴权过期（HTTP {status}），刷新后重试 {attempt} 次"));
                    self.refresh_auth()?;
                }
                Err(e) if attempt < self.retries => {
                    attempt += 1;
                    progress.note(&format!("下载失败（{e}），重试 {attempt} 次"));
                    std::thread::sleep(std::time::Duration::from_secs(1 << attempt));
                }
                Err(e) => {
                    // 收尾状态行；错误仍照原样上抛。
                    progress.file_end(label, FileOutcome::Failed);
                    return Err(e);
                }
            }
        }
    }

    /// 遍历整份清单。
    pub fn run(
        &mut self,
        root: &Path,
        list: &filelist::FileList,
        progress: &mut dyn Progress,
    ) -> Result<Report, CdnError> {
        let total_bytes: u64 = list.files.iter().map(|f| f.size).sum();
        progress.begin(list.files.len(), total_bytes);

        let mut report = Report {
            total: list.files.len(),
            ..Default::default()
        };
        for entry in &list.files {
            if is_encrypted_file(&entry.path) {
                continue; // 密文文件不按明文清单校验
            }
            let dest = list.local_path(root, entry);
            let outcome = self.fetch_one(list, entry, &dest, progress)?;
            match outcome {
                FileOutcome::Skipped => report.skipped += 1,
                FileOutcome::Downloaded => {
                    report.downloaded += 1;
                    report.bytes += entry.size;
                }
                // `run` 里不会出现：失败会直接 `?` 上抛。
                FileOutcome::Failed => {}
            }
            progress.file_end(&entry.path, outcome);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_meta_matching() {
        assert!(is_encrypted_file("game\\LocalVersion3.xml"));
        assert!(is_encrypted_file("game/LocalVersion3.xml"));
        assert!(!is_encrypted_file("game\\ffxivgame.ver"));
    }
}
