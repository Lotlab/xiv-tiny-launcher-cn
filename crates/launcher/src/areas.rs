//! 区服表策略（成功覆盖写本地备份，失败回退本地缓存）+ 选区交互
//!（`--area` → 上次大区 → 数字菜单）。

use proto::{log, paths};

use crate::consts::FILE_SERVER;

use sdo_client::server::{ServerTable, SubArea};
use sdo_client::{Api, GAME_APP_ID};

use crate::error::{Error, Result};
use crate::ui;

/// 拉取区服表；成功后覆盖写本地备份，失败回退本地缓存。
pub fn fetch_table(api: &Api) -> Result<ServerTable> {
    // 表服务的身份取游戏应用。
    match api.server_json(GAME_APP_ID) {
        Ok(fetched) => {
            let file = paths::cwd_file(FILE_SERVER);
            if let Err(e) = paths::write_atomic(&file, &fetched.raw) {
                log::debug(&format!("区服表备份写盘失败：{e}"));
            }
            Ok(fetched.table)
        }
        Err(e) => {
            log::debug(&format!("区服接口请求细节：{}", e.log_text()));
            log::warn("区服表异常，已使用本地缓存继续");
            local_table()
        }
    }
}

fn local_table() -> Result<ServerTable> {
    let file = paths::cwd_file(FILE_SERVER);
    let bytes = std::fs::read(&file)
        .map_err(|e| Error::msg(format!("无网络且本地 {} 不可读: {e}", file.display())))?;
    Ok(sdo_client::server::parse_server_json(&String::from_utf8_lossy(
        &bytes,
    ))?)
}

/// 选区优先级：`--area` → 上次记住的大区（`device.json` 的 `lastAreaId`）→ 数字菜单。
pub struct AreaPick {
    pub area: SubArea,
    pub from_last: bool,
}

pub fn resolve_area(
    table: &ServerTable,
    arg: Option<&str>,
    last_area_id: Option<&str>,
) -> Result<AreaPick> {
    // 文本来源（--area / lastAreaId）在这里解析成整数 id。
    if let Some(raw) = arg {
        let found = raw.trim().parse::<i32>().ok().and_then(|id| table.find(id));
        return found.cloned().map(|a| AreaPick { area: a, from_last: false }).ok_or_else(|| {
            Error::msg(format!(
                "--area {raw} 不在区服表中（可用: {}）",
                available(table)
            ))
        });
    }
    if let Some(raw) = last_area_id {
        match raw.trim().parse::<i32>().ok().and_then(|id| table.find(id)) {
            Some(a) => return Ok(AreaPick { area: a.clone(), from_last: true }),
            None => {
                // 记住的大区已从区服表消失：只提示，回退菜单（不清除记录）
                println!("上次的大区 [{raw}] 不在当前区服表中，请重新选择");
            }
        }
    }
    let lines: Vec<String> = table.sub_areas.iter().map(|a| a.menu_line()).collect();
    let allowed: Vec<String> = table.sub_areas.iter().map(|a| a.id.to_string()).collect();
    let picked = ui::pick_area(&lines, &allowed).ok_or_else(|| Error::msg("未选择子区，退出"))?;
    picked
        .trim()
        .parse::<i32>()
        .ok()
        .and_then(|id| table.find(id))
        .cloned()
        .map(|a| AreaPick { area: a, from_last: false })
        .ok_or_else(|| Error::msg(format!("子区 {picked} 解析失败")))
}

fn available(table: &ServerTable) -> String {
    table
        .sub_areas
        .iter()
        .map(|a| a.id.to_string())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> ServerTable {
        let text = r#"{"data":{"areaInfos":[{"subArea":[
{"id":"8","name":"豆豆柴","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"ffxivlobby08.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.GMServerHost\":\"ffxivgm08.ff14.sdo.com\",\"Dev.SaveDataBankHost\":\"ffxivsdb08.ff14.sdo.com\",\"resetConfig\":\"0\"}"},
{"id":"7","name":"猫小胖","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"ffxivlobby07.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.GMServerHost\":\"ffxivgm07.ff14.sdo.com\",\"Dev.SaveDataBankHost\":\"ffxivsdb07.ff14.sdo.com\",\"resetConfig\":\"0\"}"}
]}]}}"#;
        sdo_client::server::parse_server_json(text).unwrap()
    }

    /// `--area` 优先级最高，且不标记为"来自上次"。
    #[test]
    fn explicit_area_wins() {
        let pick = resolve_area(&table(), Some("8"), Some("7")).unwrap();
        assert_eq!(pick.area.id, 8);
        assert!(!pick.from_last);
    }

    /// 没传 `--area` 时用上次记住的大区。
    #[test]
    fn falls_back_to_remembered_area() {
        let pick = resolve_area(&table(), None, Some("7")).unwrap();
        assert_eq!(pick.area.id, 7);
        assert!(pick.from_last);
    }

    /// `--area` 指到区服表里不存在的 id → 报错，不静默回退到别的大区。
    #[test]
    fn unknown_area_is_rejected() {
        let t = table();
        assert!(t.find(6).is_none(), "夹具里不应有 6 区");
        assert!(
            resolve_area(&t, Some("6"), None).is_err(),
            "非法 --area 必须报错"
        );
    }
}
