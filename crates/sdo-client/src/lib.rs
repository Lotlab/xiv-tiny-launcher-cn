//! SDO 网络交互库：EXE 侧唯一的网络发起方（纯网络 + 纯内存解析，无文件 IO）。
//!
//! - [`Client`]：一次登录会话的网络身份
//!   （[`Identity`] 快照 + `runTimeId` + 应用口径 [`App`]），
//!   所有网络交互都是它的方法；调用方只传类型化参数，不拼 query 字符串。
//!   登录应用与游戏应用是同一种 [`App`]：前者是客户端的默认口径，后者按次传入。
//! - [`endpoint`]：每个服务端 API 是一个端点结构体（`HOST` + `TIMEOUT` + `path()`）。
//! - [`resp`]：响应解析与 `CODEKEY` 提取。
//! - [`server`]：区服表解析与游戏命令行拼接（纯内存，无落盘）。
//! - [`tickets`]：登录票据（T0）与游戏票据（T1）类型。
//! - [`error`]：失败分类与用户文案/排查细节分离；上层的重试/回退只看
//!   [`Error::is_retryable`]，面向用户的输出只看 [`Error::user`]（= `Display`）。
//!
//! 本库不持有进程级全局状态：不读写任何文件（`device.json/server.json` 落盘与回退由
//! EXE 侧完成），不做 UI（二维码渲染/按键/选区菜单在 EXE 侧）。唯一的实例状态是
//! [`Client`] 自己发出的后台附属请求句柄（[`Client::wait_pending`]）。

pub mod areas;
pub mod auxreq;
pub mod client;
pub mod endpoint;
pub mod error;
pub mod login;
pub mod resp;
pub mod server;
pub mod sso;
pub mod tickets;

pub use client::{Client, Identity, Resp, Timeout};
pub use endpoint::{App, GAME_APP, LOGIN_APP};
pub use error::{Error, Result};
pub use tickets::{GameTicket, LoginTicket};
