//! 区服表解析与游戏命令行拼接。
//!
//! 运行时以官方接口为准，本地 `./server.json` 仅断网备用。

use serde_json::Value;

use proto::consts::{DEFAULT_LOBBY_PORT, GAME_APP_ID};
use proto::log;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub lobby_host: String,
    pub lobby_port: String,
    pub gm_host: String,
    pub sdb_host: String,
    pub reset_config: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubArea {
    pub id: String,
    pub name: String,
    /// 非空时优先用它（形如 `host:port`），否则用 `meta` 里的 Lobby 入口。
    pub domain: String,
    pub meta: Option<Meta>,
}

impl SubArea {
    /// 缺失的映射键：缺任一键即中止并列出。
    pub(crate) fn missing_keys(&self) -> Vec<&'static str> {
        let m = match &self.meta {
            Some(m) => m,
            None => return vec!["meta"],
        };
        let mut v = Vec::new();
        if m.lobby_host.is_empty() {
            v.push("Dev.LobbyHost01");
        }
        if m.gm_host.is_empty() {
            v.push("Dev.GMServerHost");
        }
        if m.sdb_host.is_empty() {
            v.push("Dev.SaveDataBankHost");
        }
        if m.reset_config.is_empty() {
            v.push("resetConfig");
        }
        v
    }

    /// 区服入口 host/port：`domain` 非空优先（按最后一个 `:` 切），否则 `LobbyHost` + `LobbyPort01/54994`。
    pub(crate) fn lobby_endpoint(&self) -> Option<(String, String)> {
        let m = self.meta.as_ref()?;
        if !self.domain.trim().is_empty() {
            return Some(split_host_port(&self.domain, DEFAULT_LOBBY_PORT));
        }
        if m.lobby_host.trim().is_empty() {
            return None;
        }
        let port = if m.lobby_port.trim().is_empty() {
            DEFAULT_LOBBY_PORT
        } else {
            m.lobby_port.trim()
        };
        Some(split_host_port(&m.lobby_host, port))
    }

    /// 选区菜单的一行：用户只需要认名字，其余元数据（lobby/GM/domain/open）不展示。
    pub fn menu_line(&self) -> String {
        format!("  [{}] {}", self.id, self.name)
    }
}

/// 按最后一个 `:` 切分 `host:port`；无 `:` 时用 `default_port`。
pub(crate) fn split_host_port(s: &str, default_port: &str) -> (String, String) {
    let t = s.trim();
    match t.rfind(':') {
        Some(i) => (t[..i].to_string(), t[i + 1..].to_string()),
        None => (t.to_string(), default_port.to_string()),
    }
}

#[derive(Debug, Clone)]
pub struct ServerTable {
    pub sub_areas: Vec<SubArea>,
}

impl ServerTable {
    pub fn find(&self, area_id: &str) -> Option<&SubArea> {
        self.sub_areas.iter().find(|a| a.id == area_id)
    }
}

fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 解析 `server.json`：`data.areaInfos[].subArea[]`，`meta` 为字符串需二次解析。
pub fn parse_server_json(text: &str) -> Result<ServerTable, String> {
    const BROKEN: &str = "区服列表已损坏，请删除 server.json 后重试（将自动重新下载）";
    let root: Value = serde_json::from_str(text).map_err(|e| {
        log::debug(&format!("区服表解析细节：{e}"));
        BROKEN.to_string()
    })?;
    let area_infos = root
        .get("data")
        .and_then(|d| d.get("areaInfos"))
        .and_then(|a| a.as_array())
        .ok_or_else(|| {
            log::debug("区服表解析细节：缺少 data.areaInfos");
            BROKEN.to_string()
        })?;
    let mut sub_areas = Vec::new();
    for ai in area_infos {
        let subs = match ai.get("subArea").and_then(|x| x.as_array()) {
            Some(s) => s,
            None => continue,
        };
        for sa in subs {
            let meta_raw = sa.get("meta").and_then(|m| m.as_str()).unwrap_or("");
            let meta = parse_meta(meta_raw);
            sub_areas.push(SubArea {
                id: s(sa, "id"),
                name: s(sa, "name"),
                domain: s(sa, "domain"),
                meta,
            });
        }
    }
    if sub_areas.is_empty() {
        return Err("区服列表已损坏，请删除 server.json 后重试（将自动重新下载）".to_string());
    }
    Ok(ServerTable { sub_areas })
}

fn parse_meta(raw: &str) -> Option<Meta> {
    if raw.trim().is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(raw).ok()?;
    Some(Meta {
        lobby_host: s(&v, "Dev.LobbyHost01"),
        lobby_port: s(&v, "Dev.LobbyPort01"),
        gm_host: s(&v, "Dev.GMServerHost"),
        sdb_host: s(&v, "Dev.SaveDataBankHost"),
        reset_config: s(&v, "resetConfig"),
    })
}

/// 游戏命令行（字段顺序逐字固定）。
pub fn build_base(area: &SubArea) -> Result<String, String> {
    let missing = area.missing_keys();
    if !missing.is_empty() {
        let keys = missing.join(", ");
        log::error(&format!("子区 {} 缺少 meta 映射键: {keys}", area.id));
        return Err(format!(
            "无法进入大区 {}：区服配置缺少必要字段，请稍后重试或更新启动器",
            area.id
        ));
    }
    // missing_keys() 为空 ⇒ meta 存在且 lobby_host 非空，因此下面两处必有值。
    let meta = area.meta.as_ref().expect("missing_keys 已保证 meta 存在");
    let (host, port) = area
        .lobby_endpoint()
        .expect("missing_keys 已保证 Lobby 入口存在");
    Ok(format!(
        "-AppID={} -AreaID={} Dev.LobbyHost01={} Dev.LobbyPort01={} Dev.GMServerHost={} Dev.SaveDataBankHost={} resetConfig={} DEV.MaxEntitledExpansionID=1",
        GAME_APP_ID, area.id, host, port, meta.gm_host, meta.sdb_host, meta.reset_config
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"data":{"areaInfos":[{"id":"1","meta":"{}","name":"默认大区","order":1,"subArea":[
{"domain":"","id":"8","ip":"","meta":"{\"AppID\":\"100001900\",\"AreaID\":\"8\",\"Dev.GMServerHost\":\"ffxivgm08.ff14.sdo.com\",\"Dev.LobbyHost01\":\"ffxivlobby08.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.SaveDataBankHost\":\"ffxivsdb08.ff14.sdo.com\",\"resetConfig\":\"0\"}","name":"豆豆柴","open":1,"order":1,"status":"空闲","statusCode":1},
{"domain":"211.27.89.22:35560","id":"6","ip":"","meta":"{\"AppID\":\"100001900\",\"AreaID\":\"6\",\"Dev.GMServerHost\":\"ffxivgm05.ff14.sdo.com\",\"Dev.LobbyHost01\":\"ffxivlobby05.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.SaveDataBankHost\":\"ffxivsdb05.ff14.sdo.com\",\"resetConfig\":\"0\"}","name":"莫古力","open":1,"order":2,"status":"空闲","statusCode":1},
{"domain":"","id":"7","ip":"","meta":"{\"AppID\":\"100001900\",\"AreaID\":\"7\",\"Dev.GMServerHost\":\"ffxivgm07.ff14.sdo.com\",\"Dev.LobbyHost01\":\"ffxivlobby07.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.SaveDataBankHost\":\"ffxivsdb07.ff14.sdo.com\",\"resetConfig\":\"0\"}","name":"猫小胖","open":1,"order":3,"status":"空闲","statusCode":1},
{"domain":"211.17.89.22:35560","id":"1","ip":"","meta":"{\"AppID\":\"100001900\",\"AreaID\":\"1\",\"Dev.GMServerHost\":\"ffxivgm01.ff14.sdo.com\",\"Dev.LobbyHost01\":\"ffxivlobby01.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.SaveDataBankHost\":\"ffxivsdb01.ff14.sdo.com\",\"resetConfig\":\"0\"}","name":"陆行鸟","open":1,"order":4,"status":"空闲","statusCode":1}]}]},"resultCode":0,"resultMsg":"success"}"#;

    #[test]
    fn parse_and_menu() {
        let t = parse_server_json(FIXTURE).unwrap();
        assert_eq!(t.sub_areas.len(), 4);
        let a7 = t.find("7").unwrap();
        assert_eq!(a7.name, "猫小胖");
        assert_eq!(
            a7.lobby_endpoint().unwrap(),
            ("ffxivlobby07.ff14.sdo.com".into(), "54994".into())
        );
        // domain 非空的子区优先 domain
        let a6 = t.find("6").unwrap();
        assert_eq!(
            a6.lobby_endpoint().unwrap(),
            ("211.27.89.22".into(), "35560".into())
        );
        // 菜单只给 id + 名字，不带 lobby/GM/domain 等元数据
        assert_eq!(a6.menu_line(), "  [6] 莫古力");
    }

    #[test]
    fn base_matches_frozen_instance() {
        let t = parse_server_json(FIXTURE).unwrap();
        let base = build_base(t.find("7").unwrap()).unwrap();
        assert_eq!(
            base,
            "-AppID=100001900 -AreaID=7 Dev.LobbyHost01=ffxivlobby07.ff14.sdo.com Dev.LobbyPort01=54994 \
Dev.GMServerHost=ffxivgm07.ff14.sdo.com Dev.SaveDataBankHost=ffxivsdb07.ff14.sdo.com resetConfig=0 \
DEV.MaxEntitledExpansionID=1"
        );
    }

    #[test]
    fn missing_meta_key_aborts_with_list() {
        let text = r#"{"data":{"areaInfos":[{"subArea":[{"id":"9","name":"x","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"h\"}"}]}]}}"#;
        let t = parse_server_json(text).unwrap();
        let a = t.find("9").unwrap();
        let missing = a.missing_keys();
        assert!(missing.contains(&"Dev.GMServerHost"), "{missing:?}");
        assert!(missing.contains(&"Dev.SaveDataBankHost"), "{missing:?}");
        assert!(missing.contains(&"resetConfig"), "{missing:?}");
        // 面向用户的错误串不提字段名，细节留在日志里。
        let err = build_base(a).unwrap_err();
        assert!(!err.contains("Dev."), "{err}");
    }

    #[test]
    fn host_port_split_uses_last_colon() {
        assert_eq!(
            split_host_port("211.27.89.22:35560", "54994"),
            ("211.27.89.22".into(), "35560".into())
        );
        assert_eq!(split_host_port("h", "54994"), ("h".into(), "54994".into()));
    }
}
