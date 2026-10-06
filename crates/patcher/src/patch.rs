//! 差分包（patch）：链选择、补丁清单、zip 解压、`patch_delta_direct.dat` 解析。
//!
//! 流程（对应参考实现 `version_chain.rs` / `patch_downloader.rs`）：
//!
//! 1. `ver2.packages` 是一条 `from → to` 的线性链，从本地 internal 版本沿链走到目标。
//! 2. 每跳的 `fileListUrl` 给出一份 JSON：`{baseUrl, directFileUrl, fileList:[{url,md5,size}]}`，
//!    即若干 patch zip。
//! 3. zip 里有 `patch_delta_direct.dat`（XML 元数据）与 `Pkg\game\*.delta`。
//! 4. 元数据给出每个文件的 `origin_md5`（打前）/ `delta_md5` / `result_md5`（打后）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cdn::ver2::Ver2;
use crate::relpath::safe_rel_path;

/// 一跳差分包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchInfo {
    /// 源 internal 版本。
    pub from: String,
    /// 目标 internal 版本。
    pub to: String,
    /// 源 display 版本（用于日志）。
    pub from_display: String,
    /// 目标 display 版本。
    pub to_display: String,
    /// 相对 `base_url` 的补丁清单路径。
    pub file_list_url: String,
    /// `ver2.base_url`。
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchError {
    /// 链走不通。
    NoChain { from: String, to: String },
    /// 跳数超过上限。
    TooManyHops { from: String, to: String, max: usize },
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::NoChain { from, to } => {
                write!(f, "没有从 {from} 到 {to} 的补丁链（本地版本过旧或 CDN 无对应包）")
            }
            PatchError::TooManyHops { from, to, max } => {
                write!(f, "从 {from} 到 {to} 需要超过 {max} 跳，放弃")
            }
        }
    }
}

impl std::error::Error for PatchError {}

/// 从 `local_internal` 沿 `from → to` 走到 `target_internal`。
///
/// `local_display` 只用于日志/展示；链上每一跳的目标 display 取自包的 `versionView`。
pub fn patch_chain(
    ver2: &Ver2,
    local_internal: &str,
    local_display: &str,
    target_internal: &str,
    max_hops: usize,
) -> Result<Vec<PatchInfo>, PatchError> {
    let by_from: HashMap<&str, &crate::cdn::ver2::Package> =
        ver2.packages.iter().map(|p| (p.from.as_str(), p)).collect();

    let mut chain = Vec::new();
    let mut current = local_internal.to_string();
    let mut current_display = local_display.to_string();

    while current != target_internal {
        if chain.len() >= max_hops {
            return Err(PatchError::TooManyHops {
                from: local_internal.to_string(),
                to: target_internal.to_string(),
                max: max_hops,
            });
        }
        let pkg = by_from.get(current.as_str()).ok_or_else(|| PatchError::NoChain {
            from: current.clone(),
            to: target_internal.to_string(),
        })?;
        let to_display = pkg.display_version().to_string();
        chain.push(PatchInfo {
            from: current.clone(),
            to: pkg.to.clone(),
            from_display: current_display.clone(),
            to_display: to_display.clone(),
            file_list_url: pkg.file_list_url.clone(),
            base_url: ver2.base_url.clone(),
        });
        current = pkg.to.clone();
        current_display = to_display;
    }
    Ok(chain)
}

/// 补丁清单 JSON。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PatchFileList {
    #[serde(rename = "baseUrl", default)]
    pub base_url: String,
    #[serde(rename = "directFileUrl", default)]
    pub direct_file_url: String,
    #[serde(rename = "fileList", default)]
    pub files: Vec<PatchZip>,
}

/// 一个 patch zip。
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PatchZip {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub md5: String,
    /// 有的响应里 `size` 是数字，有的是字符串。
    #[serde(default, deserialize_with = "de_u64_flexible")]
    pub size: u64,
}

fn de_u64_flexible<'de, D>(d: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(u64),
        Str(String),
    }
    Ok(match NumOrStr::deserialize(d)? {
        NumOrStr::Num(n) => n,
        NumOrStr::Str(s) => s.parse().unwrap_or(0),
    })
}

impl PatchFileList {
    /// 某个 zip 的完整 URL。
    pub fn zip_url(&self, zip: &PatchZip) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), zip.url.trim_start_matches('/'))
    }

    /// 某个 zip 落盘的文件名（相对 `_update/zips/`）。
    ///
    /// `zip.url` 来自网络，可能是 `../../x` 之类；不安全时返回 `None`。
    pub fn zip_name(zip: &PatchZip) -> Option<String> {
        safe_rel_path(&zip.url)
    }
}

/// 一个 delta 条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeltaEntry {
    /// 相对 `game/` 的路径（不含 `.delta`），如 `ffxivgame.ver`。
    pub rel_path: String,
    /// 打之前源文件的 MD5（大写）。
    pub origin_md5: String,
    /// delta 文件自身的 MD5（大写）。
    pub delta_md5: String,
    /// 打之后结果文件的 MD5（大写）。
    pub result_md5: String,
}

impl DeltaEntry {
    /// delta 文件相对 `delta_dir` 的路径（`Pkg/game/...`）。
    pub fn delta_rel(&self) -> String {
        format!("Pkg/game/{}.delta", self.rel_path)
    }

    /// 源/结果文件相对 `game/` 的路径。
    pub fn file_rel(&self) -> String {
        self.rel_path.clone()
    }
}

/// 解析 `patch_delta_direct.dat`（XML，`<XxxSubItem Key=.. Value=../>`）。
pub fn parse_patch_metadata(xml: &[u8]) -> Result<Vec<DeltaEntry>, String> {
    let sections = parse_sub_items(xml)?;
    let path_info = sections.get("DeltaPathInfo").cloned().unwrap_or_default();
    let delta_md5 = to_map(sections.get("DeltaMD5Info"));
    let origin_md5 = to_map(sections.get("OriginMD5Info"));
    let result_md5 = to_map(sections.get("ResultMD5Info"));

    let mut out = Vec::new();
    for (key, value) in path_info {
        // value 形如 `Pkg\game\ffxivgame.ver.delta`；去掉 `Pkg\game\` 得到 game 相对路径。
        let rel_with_delta = normalize_rel(&value);
        let rel_path = match rel_with_delta.strip_suffix(".delta") {
            Some(s) => s.to_string(),
            None => rel_with_delta,
        };
        // 落盘时会 join 到 `game/` 下，所以先过一遍越界校验。
        let Some(rel_path) = safe_rel_path(&rel_path) else {
            proto::log::warn(&format!("补丁元数据里的路径不安全，已跳过：{value}"));
            continue;
        };
        let delta_key = format!("{key}.delta");
        let (Some(origin), Some(delta), Some(result)) = (
            origin_md5.get(&key),
            delta_md5.get(&delta_key),
            result_md5.get(&key),
        ) else {
            continue;
        };
        out.push(DeltaEntry {
            rel_path,
            origin_md5: origin.clone(),
            delta_md5: delta.clone(),
            result_md5: result.clone(),
        });
    }
    Ok(out)
}

fn normalize_rel(value: &str) -> String {
    let v = value.replace('\\', "/");
    let v = v.strip_prefix("Pkg/game/").unwrap_or(&v);
    v.trim_start_matches('/').to_string()
}

fn to_map(items: Option<&Vec<(String, String)>>) -> HashMap<String, String> {
    items
        .map(|v| v.iter().cloned().collect())
        .unwrap_or_default()
}

/// 解析所有 `<*SubItem Key Value/>`，按所在 section 分组。
fn parse_sub_items(xml: &[u8]) -> Result<HashMap<String, Vec<(String, String)>>, String> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut out: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut section = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if !name.eq_ignore_ascii_case("XMLROOT") {
                    section = name;
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if name.ends_with("SubItem") && !section.is_empty() {
                    let mut key = None;
                    let mut value = None;
                    for a in e.attributes().flatten() {
                        let v = a.unescape_value().map(|c| c.into_owned()).unwrap_or_default();
                        if a.key.as_ref().eq_ignore_ascii_case(b"Key") {
                            key = Some(v);
                        } else if a.key.as_ref().eq_ignore_ascii_case(b"Value") {
                            value = Some(v);
                        }
                    }
                    if let (Some(k), Some(v)) = (key, value) {
                        out.entry(section.clone()).or_default().push((k, v));
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(out)
}

/// 从 zip 里解出 `patch_delta_direct.dat` 与所有 `Pkg\game\*.delta`，并解析元数据。
///
/// 返回解出的元数据条目；delta 落在 `delta_dir/Pkg/game/...`。
///
/// **单个条目失败不终止整次更新**：不支持的解压方法 / 条目损坏只记 warning，
/// 该文件的 delta 缺席后由调用方在链尾按目标清单整文件补下。
pub fn extract_deltas(zip_paths: &[PathBuf], delta_dir: &Path) -> Result<Vec<DeltaEntry>, String> {
    std::fs::create_dir_all(delta_dir).map_err(|e| e.to_string())?;

    let mut metadata: Option<Vec<u8>> = None;
    let mut extracted = 0usize;

    for zip_path in zip_paths {
        let file = std::fs::File::open(zip_path).map_err(|e| format!("{}: {e}", zip_path.display()))?;
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| format!("{}: {e}", zip_path.display()))?;

        for i in 0..archive.len() {
            if let Err(e) = extract_one(&mut archive, i, delta_dir, &mut metadata, &mut extracted) {
                proto::log::warn(&format!(
                    "{} 第 {i} 个条目解压失败（该文件将由链尾补齐）：{e}",
                    zip_path.display()
                ));
            }
        }
    }

    let metadata = metadata.ok_or_else(|| "zip 里没有 patch_delta_direct.dat".to_string())?;
    let entries = parse_patch_metadata(&metadata)?;
    proto::log::debug(&format!("解出 {extracted} 个 delta，元数据 {} 条", entries.len()));
    Ok(entries)
}

/// 解压单个 zip 条目：`patch_delta_direct.dat` 只留第一份，`Pkg/game/*.delta` 落到 `delta_dir`。
fn extract_one<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    delta_dir: &Path,
    metadata: &mut Option<Vec<u8>>,
    extracted: &mut usize,
) -> Result<(), String> {
    let mut entry = archive.by_index(index).map_err(|e| e.to_string())?;
    if entry.is_dir() {
        return Ok(());
    }
    let Some(raw) = safe_rel_path(entry.name()) else {
        return Err(format!("条目路径不安全：{}", entry.name()));
    };

    if raw.ends_with("patch_delta_direct.dat") {
        if metadata.is_none() {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(|e| e.to_string())?;
            *metadata = Some(buf);
        }
        return Ok(());
    }

    if !raw.ends_with(".delta") || !raw.starts_with("Pkg/game/") {
        return Ok(());
    }
    let out_path = delta_dir.join(&raw);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = std::fs::File::create(&out_path).map_err(|e| e.to_string())?;
    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    *extracted += 1;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VER2: &str = r#"{
      "baseUrl": "https://h/diff",
      "areas": [{"max": "0.0.0.29"}],
      "packages": [
        {"from":"0.0.0.26","to":"0.0.0.27","versionView":"2026.09.01.0000.0000_7.56","fileListUrl":"/p1.dat"},
        {"from":"0.0.0.27","to":"0.0.0.29","versionView":"2026.09.15.0000.0000_7.56","fileListUrl":"/p2.dat"}
      ]
    }"#;

    #[test]
    fn chain_walks_multiple_hops() {
        let v: Ver2 = serde_json::from_str(VER2).unwrap();
        let chain = patch_chain(&v, "0.0.0.26", "2026.09.01.0000.0000", "0.0.0.29", 32).unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].from, "0.0.0.26");
        assert_eq!(chain[0].to, "0.0.0.27");
        assert_eq!(chain[0].from_display, "2026.09.01.0000.0000");
        assert_eq!(chain[0].to_display, "2026.09.01.0000.0000");
        assert_eq!(chain[1].to_display, "2026.09.15.0000.0000");
        assert_eq!(chain[1].file_list_url, "/p2.dat");
    }

    #[test]
    fn chain_single_hop_and_up_to_date() {
        let v: Ver2 = serde_json::from_str(VER2).unwrap();
        assert_eq!(patch_chain(&v, "0.0.0.29", "x", "0.0.0.29", 32).unwrap().len(), 0);
        assert!(matches!(
            patch_chain(&v, "0.0.0.13", "x", "0.0.0.29", 32),
            Err(PatchError::NoChain { .. })
        ));
        assert!(matches!(
            patch_chain(&v, "0.0.0.26", "x", "0.0.0.29", 1),
            Err(PatchError::TooManyHops { .. })
        ));
    }

    #[test]
    fn parses_patch_file_list() {
        let json = r#"{"baseUrl":"https://h/patch","directFileUrl":"https://h/PList.txt",
          "fileList":[{"url":"/a.zip","md5":"ABC","size":1024},{"url":"b.zip","md5":"DEF","size":"2048"}]}"#;
        let fl: PatchFileList = serde_json::from_str(json).unwrap();
        assert_eq!(fl.files.len(), 2);
        assert_eq!(fl.files[0].size, 1024);
        assert_eq!(fl.files[1].size, 2048);
        assert_eq!(fl.zip_url(&fl.files[0]), "https://h/patch/a.zip");
        assert_eq!(fl.zip_url(&fl.files[1]), "https://h/patch/b.zip");
        assert_eq!(PatchFileList::zip_name(&fl.files[0]).as_deref(), Some("a.zip"));
        assert_eq!(PatchFileList::zip_name(&fl.files[1]).as_deref(), Some("b.zip"));
    }

    /// 补丁清单里的 `url` 来自网络，落盘前必须挡住越界路径。
    #[test]
    fn zip_name_rejects_unsafe_paths() {
        let z = |url: &str| PatchZip {
            url: url.into(),
            md5: String::new(),
            size: 0,
        };
        assert_eq!(PatchFileList::zip_name(&z("/a/b.zip")).as_deref(), Some("a/b.zip"));
        assert_eq!(PatchFileList::zip_name(&z("..\\..\\evil.zip")), None);
        assert_eq!(PatchFileList::zip_name(&z("C:\\evil.zip")), None);
    }

    #[test]
    fn parses_metadata() {
        let xml = r#"<XMLROOT>
        <DeltaPathInfo>
            <DeltaPathSubItem Key="game\file1.dat" Value="Pkg\game\file1.dat.delta"/>
            <DeltaPathSubItem Key="game\sub\file2.dat" Value="Pkg\game\sub\file2.dat.delta"/>
        </DeltaPathInfo>
        <DeltaMD5Info>
            <DeltaMD5SubItem Key="game\file1.dat.delta" Value="AAAA"/>
            <DeltaMD5SubItem Key="game\sub\file2.dat.delta" Value="BBBB"/>
        </DeltaMD5Info>
        <OriginMD5Info>
            <OriginMD5SubItem Key="game\file1.dat" Value="CCCC"/>
            <OriginMD5SubItem Key="game\sub\file2.dat" Value="DDDD"/>
        </OriginMD5Info>
        <ResultMD5Info>
            <ResultMD5SubItem Key="game\file1.dat" Value="EEEE"/>
            <ResultMD5SubItem Key="game\sub\file2.dat" Value="FFFF"/>
        </ResultMD5Info>
        </XMLROOT>"#;
        let mut entries = parse_patch_metadata(xml.as_bytes()).unwrap();
        entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].rel_path, "file1.dat");
        assert_eq!(entries[0].origin_md5, "CCCC");
        assert_eq!(entries[0].delta_md5, "AAAA");
        assert_eq!(entries[0].result_md5, "EEEE");
        assert_eq!(entries[1].rel_path, "sub/file2.dat");
        assert_eq!(entries[0].delta_rel(), "Pkg/game/file1.dat.delta");
    }

    #[test]
    fn metadata_skips_incomplete_entries() {
        let xml = r#"<XMLROOT>
        <DeltaPathInfo><DeltaPathSubItem Key="game\a.dat" Value="Pkg\game\a.dat.delta"/></DeltaPathInfo>
        <OriginMD5Info><OriginMD5SubItem Key="game\a.dat" Value="CC"/></OriginMD5Info>
        </XMLROOT>"#;
        assert!(parse_patch_metadata(xml.as_bytes()).unwrap().is_empty());
    }

    /// 元数据里的 `Value` 会 join 到 `game/` 下，越界条目直接丢掉。
    #[test]
    fn metadata_skips_unsafe_paths() {
        let xml = r#"<XMLROOT>
        <DeltaPathInfo>
            <DeltaPathSubItem Key="game\..\..\evil.dat" Value="Pkg\game\..\..\evil.dat.delta"/>
            <DeltaPathSubItem Key="game\ok.dat" Value="Pkg\game\ok.dat.delta"/>
        </DeltaPathInfo>
        <DeltaMD5Info>
            <DeltaMD5SubItem Key="game\..\..\evil.dat.delta" Value="AA"/>
            <DeltaMD5SubItem Key="game\ok.dat.delta" Value="BB"/>
        </DeltaMD5Info>
        <OriginMD5Info>
            <OriginMD5SubItem Key="game\..\..\evil.dat" Value="CC"/>
            <OriginMD5SubItem Key="game\ok.dat" Value="DD"/>
        </OriginMD5Info>
        <ResultMD5Info>
            <ResultMD5SubItem Key="game\..\..\evil.dat" Value="EE"/>
            <ResultMD5SubItem Key="game\ok.dat" Value="FF"/>
        </ResultMD5Info>
        </XMLROOT>"#;
        let entries = parse_patch_metadata(xml.as_bytes()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].rel_path, "ok.dat");
    }
}
