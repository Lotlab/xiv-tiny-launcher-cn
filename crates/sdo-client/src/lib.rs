//! SDO 网络交互库：EXE 侧唯一的网络发起方（纯网络 + 纯内存解析，无文件 IO）。
//!
//! 分两层，两个对象：
//!
//! - **层 1 [`Api`]**：接口封装，一个方法 = 一个服务端接口；内部持登录链的服务端值
//!   （`guid` / `codeKey` / `tgt` / `authorization`）。只做「发请求 + 解析 + 记状态」，
//!   不含循环、回退、重试、落盘、UI。
//! - **层 2 [`Flow`]**：FFXIV 启动器的流程封装 —— 登录链、换票、人脸验证、附属请求。
//!   循环与业务规则都在这里；UI 通过 [`Ui`] 反向调用，落盘与起进程由 EXE 做。
//!
//! 其余模块都是这两层的零件：[`endpoint`](crate) 与 `transport` 是层 1 的内部，
//! `server` 是 `server.json` 的 schema，[`Error`] 是错误分类。
//!
//! 本库不持有进程级全局状态：不读写任何文件（`device.json` / `server.json` 的落盘与
//! 回退由 EXE 完成），不做 UI（二维码渲染/按键/选区菜单在 EXE 侧）。

mod api;
mod consts;
mod endpoint;
mod error;
mod flow;
mod resp;
pub mod server;
mod tickets;
mod transport;
mod ui;

pub use api::{Api, FaceVerify, FastLogin, FetchedTable, Poll, ProbePaths, QrCode, Request};
pub use endpoint::{App, GAME_APP, LOGIN_APP};
pub use error::{Error, Kind, Result};
pub use flow::{Flow, KeepKey, Method, Policy};
pub use tickets::GameTicket;
pub use transport::Identity;
pub use ui::{Action, Note, Phase, Ui, Wait};
