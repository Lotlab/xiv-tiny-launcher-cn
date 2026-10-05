//! sdo-client 的字面值总表：协议表 + 流程默认值。
//!
//! 只被本 crate 与 EXE 用；EXE↔DLL 的 ABI 常量在 `proto::consts`。
//! 应用口径（两个 App 的 `appId`、版本号等）是 `LOGIN_APP` / `GAME_APP` 两个常量。

// ── 协议表 ──

// 服务端 host：固定值，不自动切换。
pub(crate) const HOST_CAS: &str = "cas.sdo.com";
pub(crate) const HOST_N_CAS: &str = "n.cas.sdo.com";
pub(crate) const HOST_GFC: &str = "gfc.sdo.com";
pub(crate) const HOST_UTILITY: &str = "utility.sdoprofile.com";
pub(crate) const HOST_V3LAUNCHER: &str = "v3launcher.jijiagames.com";

// 自设 UA/Accept；Host 每次按目标主机填入。不设 Referer/Expect。
pub(crate) const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; Trident/7.0; rv:11.0) like Gecko sdologin-Launcher";
pub(crate) const ACCEPT: &str = "*/*";

// 固定公共参数。
pub(crate) const AUTHEN_SOURCE: &str = "1";
pub(crate) const LOCALE: &str = "zh_CN";
pub(crate) const PRODUCT_ID: i32 = 4;
// 下面三个在官方模板里是内联字面量，不是 `%d` 实参，故保持字符串。
pub(crate) const FRAME_TYPE: &str = "1";
pub(crate) const ENDPOINT_OS: &str = "1";
pub(crate) const VERSION: &str = "21";
pub(crate) const CUSTOM_SECURITY_LEVEL: i32 = 2;
pub(crate) const THIRD_LOGIN_EXTERN: &str = "0";

/// 服务端要求的 `tag` 取值。
pub(crate) const TAG: i32 = 0;
pub(crate) const CHANNEL_ID: &str = "0";

/// CAS 认证超时（连接超时与总超时相同）。
pub(crate) const TIMEOUT_AUTH_MS: u64 = 5000;
/// 下载类超时（server.json / 二维码 / 附属请求）。
pub(crate) const TIMEOUT_DOWNLOAD_MS: u64 = 10_000;

/// 勾选：首包发 1（默认）。
pub(crate) const KEEP_LOGIN_FLAG_CHECKED: i32 = 1;
/// 未勾选：首包发 -1。`0` 永不发送。
pub(crate) const KEEP_LOGIN_FLAG_UNSET: i32 = -1;

// 服务端返回码：仅列出具名分支用到的几个，其余一律透传原文。
pub(crate) const RC_OK: i64 = 0;
pub(crate) const RC_QR_NOT_SCANNED: i64 = -10515805;
pub(crate) const RC_PUSH_NOT_CONFIRMED: i64 = -10516808;

/// 区服表里缺 `LobbyPort` 时的兜底端口。
pub(crate) const DEFAULT_LOBBY_PORT: &str = "54994";

// ── 流程默认值（`Policy` 的各项；EXE 的 CLI 也拿它们当默认）──

pub const DEFAULT_QR_TIMEOUT_SECS: u64 = 120;
pub const DEFAULT_QR_MAX_ATTEMPTS: u32 = 60;
pub const DEFAULT_POLL_MIN_MS: u64 = 900;
pub const DEFAULT_POLL_MAX_MS: u64 = 1100;
/// 单码连续换码上限，防止服务端持续报错时无限重取二维码。
pub const MAX_QR_CODE_ROUNDS: u32 = 60;
/// SSO 换票连续失败后重新登录的轮数上限。
pub const MAX_LOGIN_ROUNDS: u32 = 3;
/// 退出前等待后台附属请求的总预算。
pub const AUX_WAIT_BUDGET_MS: u64 = 1500;
pub const PUSH_SEND_MAX_TRIES: u32 = 3;
pub const PUSH_SEND_RETRY_WAIT_MS: u64 = 1000;
