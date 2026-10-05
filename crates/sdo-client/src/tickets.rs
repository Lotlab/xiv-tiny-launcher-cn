//! 票据类型。
//!
//! - T0（`ticket0` / `tgt0` / `guid0`）：登录链拿到的原始票据。它只活在
//!   [`Api`](crate::Api) 内部 —— 换票要的是 `tgt0`/`guid0`，`ticket0` 本身没有任何
//!   后续请求会用到，所以不对外暴露。
//! - [`GameTicket`]：SSO 换票后交给游戏的票据（T1）。`tgt0` 沿用登录链的、只驻 EXE
//!   内存，不随 T1 交接。

/// 换票后交给游戏的票据。
#[derive(Debug, Clone)]
pub struct GameTicket {
    pub ticket: String,
    pub snda_id: String,
}
