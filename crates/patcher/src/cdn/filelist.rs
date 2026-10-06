//! `client_all_files_list.dat` 解析与单文件下载 URL。
//!
//! 首行 header：`hash_base_path|identifier|hash_id`，例如
//! `https://ff14.jijiagames.com/v3client/build/100001900/8847/apppc/1131|100001900|0.0.0.29`。
//! 之后每行：`path|size|md5`，`path` 用反斜杠（如 `game\sqpack\ffxiv\0a0000.pack`）。
//!
//! 单文件 URL = `{hash_base_path}/{目录}/{server_file_id}`，其中
//! `server_file_id = MD5_UTF16LE("{identifier}_{hash_id}_{path}")`（大写 hex）。

use crate::hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// 相对安装根（安装根 = `game/` 的父目录），反斜杠分隔。
    pub path: String,
    pub size: u64,
    /// 大写 MD5。
    pub hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileList {
    pub hash_base_path: String,
    pub identifier: String,
    pub hash_id: String,
    pub files: Vec<FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileListError {
    Empty,
    BadHeader,
}

impl std::fmt::Display for FileListError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileListError::Empty => write!(f, "文件清单为空"),
            FileListError::BadHeader => write!(f, "文件清单首行格式非法"),
        }
    }
}

impl std::error::Error for FileListError {}

/// 解析 `client_all_files_list.dat`。
pub fn parse_file_list(raw: &str) -> Result<FileList, FileListError> {
    let mut lines = raw.lines();
    let header = lines.next().ok_or(FileListError::Empty)?;
    let mut parts = header.split('|');
    let hash_base_path = parts.next().unwrap_or("").to_string();
    let identifier = parts.next().unwrap_or("").to_string();
    let hash_id = parts.next().unwrap_or("").to_string();
    if hash_base_path.is_empty() || identifier.is_empty() || hash_id.is_empty() {
        return Err(FileListError::BadHeader);
    }

    let mut files = Vec::new();
    for line in lines {
        let mut f = line.split('|');
        let (Some(path), Some(size), Some(hash)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if path.is_empty() || hash.is_empty() {
            continue;
        }
        // `path` 会被 `local_path` 直接 join 到安装根下；越界路径整行丢掉。
        // 注意：不能改写成 `/` 分隔后存起来——`server_file_id` 要按**原始**路径算 MD5。
        if !crate::relpath::is_safe_local_rel(path) {
            proto::log::warn(&format!("文件清单里的路径不安全，已跳过：{path}"));
            continue;
        }
        files.push(FileEntry {
            path: path.to_string(),
            size: size.parse().unwrap_or(0),
            hash: hash.to_string(),
        });
    }

    Ok(FileList {
        hash_base_path,
        identifier,
        hash_id,
        files,
    })
}

/// `MD5_UTF16LE("{identifier}_{hash_id}_{path}")`（大写）。
pub fn server_file_id(identifier: &str, hash_id: &str, file_path: &str) -> String {
    let concat = format!("{identifier}_{hash_id}_{file_path}");
    let utf16le: Vec<u8> = concat.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    hash::md5_hex_upper(&utf16le)
}

/// 单文件下载 URL。
pub fn download_url(hash_base_path: &str, file_path: &str, identifier: &str, hash_id: &str) -> String {
    let id = server_file_id(identifier, hash_id, file_path);
    let base = hash_base_path.trim_end_matches('/');
    match file_path.rfind('\\') {
        Some(i) => format!("{base}/{}/{}", file_path[..i].replace('\\', "/"), id),
        None => format!("{base}/{id}"),
    }
}

impl FileList {
    /// 某个条目在安装根下的落盘路径（反斜杠转 `/`）。
    ///
    /// 路径来自 [`parse_file_list`]，已保证是安全的相对路径（无 `..` / 盘符 / 前导分隔符）。
    pub fn local_path(&self, root: &std::path::Path, entry: &FileEntry) -> std::path::PathBuf {
        debug_assert!(
            crate::relpath::is_safe_local_rel(&entry.path),
            "清单路径应已校验：{}",
            entry.path
        );
        root.join(entry.path.replace('\\', "/"))
    }

    /// 某个条目的下载 URL。
    pub fn url_for(&self, entry: &FileEntry) -> String {
        download_url(&self.hash_base_path, &entry.path, &self.identifier, &self.hash_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_and_rows() {
        let raw = "https://h/base|100001900|0.0.0.29\ngame\\a.dat|123|ABCD\ngame\\b.dat|456|EF01\nshort\n";
        let fl = parse_file_list(raw).unwrap();
        assert_eq!(fl.hash_base_path, "https://h/base");
        assert_eq!(fl.identifier, "100001900");
        assert_eq!(fl.hash_id, "0.0.0.29");
        assert_eq!(fl.files.len(), 2);
        assert_eq!(fl.files[0].path, "game\\a.dat");
        assert_eq!(fl.files[0].size, 123);
        assert_eq!(fl.files[0].hash, "ABCD");
    }

    #[test]
    fn empty_or_bad_header() {
        assert_eq!(parse_file_list(""), Err(FileListError::Empty));
        assert_eq!(parse_file_list("only-one-column\n"), Err(FileListError::BadHeader));
    }

    /// 越界路径不能进清单（`local_path` 会直接 join）。
    #[test]
    fn skips_unsafe_paths_but_keeps_original_form() {
        let raw = "https://h/base|100001900|0.0.0.29\n\
                   game\\a.dat|1|AA\n\
                   ..\\..\\evil.dat|1|BB\n\
                   C:\\evil.dat|1|CC\n";
        let fl = parse_file_list(raw).unwrap();
        assert_eq!(fl.files.len(), 1);
        // 原始反斜杠形式必须保留：单文件 URL 的 MD5 按它算。
        assert_eq!(fl.files[0].path, "game\\a.dat");
    }

    /// 固定向量（外部算好写死，避免在测试里重算一遍实现）。
    #[test]
    fn server_file_id_is_utf16le_md5() {
        assert_eq!(
            server_file_id("100001900", "0.0.0.29", "game\\ffxivgame.ver"),
            "1113767C0B923103902456D56C73798B"
        );
    }

    #[test]
    fn download_url_keeps_directory() {
        assert_eq!(
            download_url(
                "https://h/base",
                "game\\sqpack\\ffxiv\\0a0000.pack",
                "100001900",
                "0.0.0.29",
            ),
            "https://h/base/game/sqpack/ffxiv/58CA0F8EB92A05462A70A38787EE216C"
        );
        // 无目录：直接拼在 base 下
        assert_eq!(
            download_url("https://h/base/", "ffxivgame.ver", "1", "2"),
            "https://h/base/531AC06F24028D339E5ED6A6D3D2B712"
        );
    }
}
