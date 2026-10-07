//! `ver2.dat`：CDN 的版本与差分包清单。
//!
//! URL：`https://{host}/v3launcher/build/ver2data/{game_id}/{build_id}/-1/ver2.dat`
//!
//! 结构（JSON）：
//! ```json
//! {
//!   "baseUrl": "https://ff14.jijiagames.com/v3client/build/100001900/8847/diff",
//!   "areas":   [ { "id":"0", "min":"0.0.0.13", "max":"0.0.0.29", "must":"0.0.0.29", "back":"0.0.0.0" } ],
//!   "packages":[ { "from":"0.0.0.27", "to":"0.0.0.29",
//!                  "versionView":"2026.09.15.0000.0000_7.56",
//!                  "fileListUrl":"/0.0.0.27-0.0.0.29-…/100001900_0.0.0.27-0.0.0.29_FileList.dat" } ]
//! }
//! ```
//!
//! `areas[0].max` 是最新 internal 版本；`versionView` 下划线前是 display 版本，
//! 下划线后是名字（如 `7.56`）。`packages` 是一条 `from → to` 的线性链。

use serde::Deserialize;

/// 一个区服（`areas` 项）。
#[derive(Debug, Clone, Deserialize)]
pub struct Area {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub min: String,
    /// 最新 internal 版本。
    #[serde(default)]
    pub max: String,
    #[serde(default)]
    pub must: String,
    #[serde(default)]
    pub back: String,
}

/// 一个差分包（`packages` 项）。
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    /// `YYYY.MM.DD.HHMM.HHMM_<name>`。
    #[serde(rename = "versionView", default)]
    pub version_view: String,
    /// 相对 `base_url` 的补丁清单路径。
    #[serde(rename = "fileListUrl", default)]
    pub file_list_url: String,
}

impl Package {
    /// `versionView` 下划线前的 display 版本。
    pub fn display_version(&self) -> &str {
        split_version_view(&self.version_view).0
    }

    /// `versionView` 下划线后的名字。
    pub fn name(&self) -> &str {
        split_version_view(&self.version_view).1
    }
}

impl RemoteVersion {
    /// 完整的 `versionView`（`display_name`，无名字时就是 display）。
    pub fn view_full(&self) -> String {
        if self.name.is_empty() {
            self.display.clone()
        } else {
            format!("{}_{}", self.display, self.name)
        }
    }
}

/// `ver2.dat` 全文。
#[derive(Debug, Clone, Deserialize)]
pub struct Ver2 {
    #[serde(rename = "baseUrl", default)]
    pub base_url: String,
    /// 备用 base（`backupBaseUrl`）：主 host 被边缘拒绝时换 host 重试。
    ///
    /// 鉴权 hash 只覆盖 path，换 host 不影响鉴权有效性。
    #[serde(rename = "backupBaseUrl", default)]
    pub backup_base_url: String,
    #[serde(default)]
    pub areas: Vec<Area>,
    #[serde(default)]
    pub packages: Vec<Package>,
}

/// CDN 当前最新版本（从 `ver2.dat` 里挑出来的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteVersion {
    /// internal 版本，如 `0.0.0.29`。
    pub internal: String,
    /// display 版本，如 `2026.09.15.0000.0000`。
    pub display: String,
    /// 名字，如 `7.56`。
    pub name: String,
}

impl Ver2 {
    /// `areas[0].max`。
    pub fn latest_internal(&self) -> Option<&str> {
        self.areas.first().map(|a| a.max.as_str()).filter(|s| !s.is_empty())
    }

    /// 备用 base 的 host：与主 base host 不同才返回（相同/缺失/非法则无备用）。
    pub fn backup_host(&self) -> Option<String> {
        let backup = self.backup_base_url.trim();
        if backup.is_empty() {
            return None;
        }
        let backup_url = url::Url::parse(backup).ok()?;
        let backup_host = backup_url.host_str()?.to_string();
        let base_url = url::Url::parse(self.base_url.trim()).ok()?;
        if base_url.host_str() == Some(backup_host.as_str()) {
            return None;
        }
        Some(backup_host)
    }

    /// 把 `to == internal` 的包挑出来，解析出 display/name。
    pub fn latest(&self) -> Result<RemoteVersion, Ver2Error> {
        let internal = self
            .latest_internal()
            .ok_or(Ver2Error::NoArea)?
            .to_string();
        let pkg = self
            .packages
            .iter()
            .find(|p| p.to == internal)
            .ok_or_else(|| Ver2Error::NoPackageFor(internal.clone()))?;
        let (display, name) = split_version_view(&pkg.version_view);
        if display.is_empty() {
            return Err(Ver2Error::EmptyVersionView(internal));
        }
        Ok(RemoteVersion {
            internal,
            display: display.to_string(),
            name: name.to_string(),
        })
    }

    /// 直接给出 `to == internal` 的包。
    pub fn package_to(&self, internal: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.to == internal)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ver2Error {
    /// `areas` 为空。
    NoArea,
    /// `packages` 里没有 `to == areas[0].max` 的包。
    NoPackageFor(String),
    /// `versionView` 为空。
    EmptyVersionView(String),
}

impl std::fmt::Display for Ver2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ver2Error::NoArea => write!(f, "ver2.dat 没有 areas"),
            Ver2Error::NoPackageFor(v) => write!(f, "ver2.dat 没有 to={v} 的包"),
            Ver2Error::EmptyVersionView(v) => write!(f, "ver2.dat 里 {v} 的 versionView 为空"),
        }
    }
}

impl std::error::Error for Ver2Error {}

/// 拆 `YYYY.MM.DD.HHMM.HHMM_<name>`；没有下划线时名字为空。
fn split_version_view(view: &str) -> (&str, &str) {
    match view.split_once('_') {
        Some((v, name)) => (v, name),
        None => (view, ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "baseUrl": "https://ff14.jijiagames.com/v3client/build/100001900/8847/diff",
      "backupBaseUrl": "https://ff14traffic1.jijiagames.com/v3client/build/100001900/8847/diff",
      "areas": [{ "back": "0.0.0.0", "id": "0", "max": "0.0.0.29", "min": "0.0.0.13", "must": "0.0.0.29" }],
      "packages": [
        { "from": "0.0.0.27", "to": "0.0.0.29",
          "versionView": "2026.09.15.0000.0000_7.56",
          "fileListUrl": "/0.0.0.27-0.0.0.29-abc/100001900_0.0.0.27-0.0.0.29_FileList.dat" },
        { "from": "0.0.0.26", "to": "0.0.0.27",
          "versionView": "2026.09.01.0000.0000_7.56",
          "fileListUrl": "/x.dat" }
      ]
    }"#;

    #[test]
    fn parses_and_picks_latest() {
        let v: Ver2 = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(v.base_url, "https://ff14.jijiagames.com/v3client/build/100001900/8847/diff");
        assert_eq!(
            v.backup_host().as_deref(),
            Some("ff14traffic1.jijiagames.com")
        );
        assert_eq!(v.latest_internal(), Some("0.0.0.29"));
        let latest = v.latest().unwrap();
        assert_eq!(latest.internal, "0.0.0.29");
        assert_eq!(latest.display, "2026.09.15.0000.0000");
        assert_eq!(latest.name, "7.56");
        assert_eq!(v.package_to("0.0.0.29").unwrap().from, "0.0.0.27");
    }

    #[test]
    fn version_view_without_underscore() {
        assert_eq!(split_version_view("7.30"), ("7.30", ""));
        assert_eq!(split_version_view("2026.09.15.0000.0000_7.56"), ("2026.09.15.0000.0000", "7.56"));
    }

    #[test]
    fn missing_package_reports_internal() {
        let v: Ver2 = serde_json::from_str(
            r#"{"areas":[{"max":"0.0.0.30"}],"packages":[]}"#,
        )
        .unwrap();
        assert_eq!(v.latest(), Err(Ver2Error::NoPackageFor("0.0.0.30".into())));
    }

    #[test]
    fn backup_host_only_when_different() {
        let v: Ver2 = serde_json::from_str(SAMPLE).unwrap();
        assert_eq!(
            v.backup_host().as_deref(),
            Some("ff14traffic1.jijiagames.com")
        );
        // 缺失 → 无备用。
        let v: Ver2 = serde_json::from_str(r#"{"baseUrl":"https://h/diff"}"#).unwrap();
        assert_eq!(v.backup_host(), None);
        // 与主 host 相同 → 无备用（换了也落到同一批节点）。
        let v: Ver2 = serde_json::from_str(
            r#"{"baseUrl":"https://h/diff","backupBaseUrl":"https://h/other"}"#,
        )
        .unwrap();
        assert_eq!(v.backup_host(), None);
        // 非法 URL → 无备用。
        let v: Ver2 = serde_json::from_str(
            r#"{"baseUrl":"https://h/diff","backupBaseUrl":"not a url"}"#,
        )
        .unwrap();
        assert_eq!(v.backup_host(), None);
    }
}
