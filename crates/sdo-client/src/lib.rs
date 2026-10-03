//! SDO 网络交互库：EXE 侧唯一的网络发起方（纯网络 + 纯内存解析，无文件 IO）。
//!
//! - [`Client`](client::Client)：一次登录会话的网络身份
//!   （[`Identity`](client::Identity) 快照 + `runTimeId` + [`LoginApp`](endpoint::LoginApp)），
//!   所有网络交互都是它的方法；调用方只传类型化参数，不拼 query 字符串。
//!   游戏应用作用域（[`GameApp`](endpoint::GameApp)）不是客户端成分，只在换票时按次传入。
//! - [`endpoint`]：每个服务端 API 是一个端点结构体（`HOST` + `TIMEOUT` + `path()`）。
//! - [`resp`]：响应解析与 `CODEKEY` 提取。
//! - [`server`]：区服表解析与游戏命令行拼接（纯内存，无落盘）。
//! - [`tickets`]：登录票据（T0）与游戏票据（T1）类型。
//!
//! 本库无状态（除 `Client` 持有的快照外）：不读写任何文件
//! （`device.json/server.json` 落盘与回退由 EXE 侧完成），
//! 不做 UI（二维码渲染/按键/选区菜单在 EXE 侧）。

pub mod areas;
pub mod auxreq;
pub mod client;
pub mod endpoint;
pub mod login;
pub mod resp;
pub mod server;
pub mod sso;
pub mod tickets;

pub use client::{Client, Identity, Resp, Timeout};
pub use endpoint::{GameApp, LoginApp};
pub use tickets::{GameTicket, LoginTicket};
