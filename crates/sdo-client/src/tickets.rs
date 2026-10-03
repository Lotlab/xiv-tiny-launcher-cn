//! 登录链与换票的票据类型（原 `launcher::ctx` 定义整体搬入本库）。
//!
//! - `LoginTicket`：登录链拿到的原始票据（T0：`ticket0/tgt0/guid0`）。
//! - `GameTicket`：SSO 换票后交给游戏的票据（T1，无 `tgt1`，沿用 T0 的 `tgt` 只驻 EXE 内存）。

/// 登录链拿到的原始票据。
#[derive(Debug, Clone)]
pub struct LoginTicket {
    pub ticket: String,
    pub tgt: String,
    pub guid: String,
}

/// 换票后交给游戏的票据。
#[derive(Debug, Clone)]
pub struct GameTicket {
    pub ticket: String,
    pub snda_id: String,
}
