//! 游戏进程命令行构造。产出的是游戏进程的 argv（不是网络协议），所以放在 EXE 侧。
//! 参数顺序逐字固定：`server.json` 里 `meta` 的映射键直接变成 `Dev.*` 参数。

use proto::log;
use sdo_client::server::SubArea;
use sdo_client::GAME_APP;

use crate::error::{Error, Result};

pub struct Builder<'a> {
    area: &'a SubArea,
}

impl<'a> Builder<'a> {
    pub fn new(area: &'a SubArea) -> Builder<'a> {
        Builder { area }
    }

    /// `-AppID=… -AreaID=… Dev.LobbyHost01=… …`
    pub fn build(&self) -> Result<String> {
        let area = self.area;
        let missing = area.missing_keys();
        if !missing.is_empty() {
            let keys = missing.join(", ");
            log::error(&format!("子区 {} 缺少 meta 映射键: {keys}", area.id));
            // 面向用户的文案不提字段名，细节留在日志里。
            return Err(Error::msg(format!(
                "无法进入大区 {}：区服配置缺少必要字段，请稍后重试或更新启动器",
                area.id
            )));
        }
        // missing_keys() 为空 ⇒ meta 存在且 Lobby 入口非空，下面两处必有值。
        let meta = area.meta.as_ref().expect("missing_keys 已保证 meta 存在");
        let (host, port) = area
            .lobby_endpoint()
            .expect("missing_keys 已保证 Lobby 入口存在");
        Ok(format!(
            "-AppID={} -AreaID={} Dev.LobbyHost01={} Dev.LobbyPort01={} Dev.GMServerHost={} Dev.SaveDataBankHost={} resetConfig={} DEV.MaxEntitledExpansionID=1",
            GAME_APP.app_id, area.id, host, port, meta.gm_host, meta.sdb_host, meta.reset_config
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sdo_client::server::{parse_server_json, ServerTable};

    fn table() -> ServerTable {
        let text = r#"{"data":{"areaInfos":[{"subArea":[
{"id":"8","name":"豆豆柴","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"ffxivlobby08.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.GMServerHost\":\"ffxivgm08.ff14.sdo.com\",\"Dev.SaveDataBankHost\":\"ffxivsdb08.ff14.sdo.com\",\"resetConfig\":\"0\"}"},
{"id":"7","name":"猫小胖","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"ffxivlobby07.ff14.sdo.com\",\"Dev.LobbyPort01\":\"54994\",\"Dev.GMServerHost\":\"ffxivgm07.ff14.sdo.com\",\"Dev.SaveDataBankHost\":\"ffxivsdb07.ff14.sdo.com\",\"resetConfig\":\"0\"}"}
]}]}}"#;
        parse_server_json(text).unwrap()
    }

    #[test]
    fn base_matches_frozen_instance() {
        let t = table();
        let base = Builder::new(t.find("7").unwrap()).build().unwrap();
        assert_eq!(
            base,
            "-AppID=100001900 -AreaID=7 Dev.LobbyHost01=ffxivlobby07.ff14.sdo.com Dev.LobbyPort01=54994 \
Dev.GMServerHost=ffxivgm07.ff14.sdo.com Dev.SaveDataBankHost=ffxivsdb07.ff14.sdo.com resetConfig=0 \
DEV.MaxEntitledExpansionID=1"
        );
    }

    #[test]
    fn missing_meta_key_aborts_without_field_names() {
        let text = r#"{"data":{"areaInfos":[{"subArea":[{"id":"9","name":"x","open":1,"status":"空闲","domain":"","meta":"{\"Dev.LobbyHost01\":\"h\"}"}]}]}}"#;
        let t = parse_server_json(text).unwrap();
        let err = Builder::new(t.find("9").unwrap()).build().unwrap_err();
        let text = err.to_string();
        assert!(text.contains("大区 9"), "{text}");
        assert!(!text.contains("Dev."), "{text}");
    }
}
