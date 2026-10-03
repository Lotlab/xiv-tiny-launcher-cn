//! 区服表拉取（纯网络：`GET + 解析`，无落盘、无回退）。
//!
//! 成功写本地备份、失败读本地缓存属于带副作用的业务策略，由 EXE 侧
//! `launcher::areas::fetch_table` 负责；选区交互同样在 EXE 侧。

use proto::consts::GAME_APP_ID;

use crate::client::{Client, Result};
use crate::endpoint::ServerJson;
use crate::server::ServerTable;

/// 区服表拉取结果：解析后的表 + 服务端原文（调用方拿原文写本地备份）。
pub struct FetchedTable {
    pub table: ServerTable,
    pub raw: Vec<u8>,
}

impl Client {
    /// 拉取区服表（`GET + 解析`，无落盘、无回退；缓存策略由调用方负责）。
    /// `Err` = 传输失败/`HTTP != 200`/解析失败。
    /// 表服务的身份取冻结常量（早于选区确定，无法取按次的 `GameApp`）。
    pub fn fetch_server_table(&self) -> Result<FetchedTable> {
        let ep = ServerJson::new(GAME_APP_ID, proto::clock::now_millis());
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Err(format!("区服接口状态码 {}", r.status));
        }
        let table = crate::server::parse_server_json(&r.text())?;
        Ok(FetchedTable {
            table,
            raw: r.body,
        })
    }
}
