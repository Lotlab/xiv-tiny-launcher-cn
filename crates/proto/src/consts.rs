//! 常量总表：URL 模板、编码、本地路径与超时共用的字面值集中在此，不散落在业务代码里。
//!
//! 应用口径（两个 App 的 `appId`、版本号等）在 `sdo-client` 的 `endpoint::LOGIN_APP` /
//! `endpoint::GAME_APP` —— 它们是 `App` 类型的常量值，依赖本层的字面值之外的类型。

// 服务端 host：固定值，不自动切换。
pub const HOST_CAS: &str = "cas.sdo.com";
pub const HOST_N_CAS: &str = "n.cas.sdo.com";
pub const HOST_GFC: &str = "gfc.sdo.com";
pub const HOST_UTILITY: &str = "utility.sdoprofile.com";
pub const HOST_V3LAUNCHER: &str = "v3launcher.jijiagames.com";

// 自设 UA/Accept；Host 每次按目标主机填入。不设 Referer/Expect。
pub const UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; Trident/7.0; rv:11.0) like Gecko sdologin-Launcher";
pub const ACCEPT: &str = "*/*";

// 固定公共参数。
pub const AUTHEN_SOURCE: &str = "1";
pub const LOCALE: &str = "zh_CN";
pub const PRODUCT_ID: &str = "4";
pub const FRAME_TYPE: &str = "1";
pub const ENDPOINT_OS: &str = "1";
pub const VERSION: &str = "21";
pub const CUSTOM_SECURITY_LEVEL: &str = "2";
pub const THIRD_LOGIN_EXTERN: &str = "0";

// 登录应用与游戏应用的 appId/版本号等口径不在这里：它们是 `sdo-client::endpoint` 的
// `LOGIN_APP` / `GAME_APP` 两个常量（与 `App` 类型放一起）。

/// 服务端要求的 `tag` 取值。
pub const TAG: i32 = 0;
pub const CHANNEL_ID: &str = "0";

// 本地策略常量（可由 CLI 覆盖）。
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

/// CAS 认证超时（连接超时与总超时相同）。
pub const TIMEOUT_AUTH_MS: u64 = 5000;
/// 下载类超时（server.json / 二维码 / 附属请求）。
pub const TIMEOUT_DOWNLOAD_MS: u64 = 10_000;

/// 勾选：首包发 1（默认）。
pub const KEEP_LOGIN_FLAG_CHECKED: i32 = 1;
/// 未勾选：首包发 -1。`0` 永不发送。
pub const KEEP_LOGIN_FLAG_UNSET: i32 = -1;

// 服务端返回码：仅列出具名分支用到的几个，其余一律透传原文。
pub const RC_OK: i64 = 0;
pub const RC_QR_NOT_SCANNED: i64 = -10515805;
pub const RC_PUSH_NOT_CONFIRMED: i64 = -10516808;
pub const RC_PUSH_EMPTY_SESSION: i64 = -10242301;

// 交接环境变量：EXE 设置，游戏侧 DLL 只读。
pub const ENV_TICKET: &str = "SDO_FFXIV_TICKET";
pub const ENV_SNDAID: &str = "SDO_FFXIV_SNDAID";
pub const ENV_AREAID: &str = "SDO_FFXIV_AREAID";
pub const ENV_BASE: &str = "SDO_FFXIV_BASE";

// 本地文件名：前三个在 EXE 当前目录，日志在 %TEMP%\SdoFfxiv。
pub const FILE_DEVICE: &str = "device.json";
pub const FILE_SERVER: &str = "server.json";
pub const FILE_QRCODE: &str = "qrcode.png";
pub const LOG_DIR_NAME: &str = "SdoFfxiv";
pub const LOG_LAUNCHER: &str = "launcher.log";
pub const LOG_DLL: &str = "sdologinentry.log";
/// 单实例锁文件名（放在每用户私有目录；见 `paths::lock_file`）。
pub const LOCK_FILE_NAME: &str = "SdoFfxiv.lock";

// 游戏侧。
pub const GAME_SUBDIR: &str = "game";
pub const GAME_EXE: &str = "ffxiv_dx11.exe";
pub const DEFAULT_LOBBY_PORT: &str = "54994";
/// 自研 DLL 的文件名；游戏按 `<exe 目录>\..\sdo\sdologin\<该名>` 动态加载。
pub const DLL_NAME: &str = "sdologinentry64.dll";
/// 自研 DLL 内的构建标记；启动器据此确认目标位置放的是本项目产物而不是官方原件。
pub const DLL_BUILD_MARKER: &str = "xiv-tiny-launcher-cn/sdologinentry64 build-marker v1";

/// Login 系列 IID：游戏实际传第二个值，其余为表值兼容。
pub const IID_LOGIN: [&str; 4] = [
    "AD887932-2D1C-48EC-B30E-535B609C12D6",
    "7B06DAD6-6832-4455-AFC6-6C8BE902534B",
    "D09EE9A2-8C44-42D6-9F3E-E34727BBEA10",
    "D09EE9A2-8C44-42D6-9FE2-E34727BBEA10",
];
pub const IID_INFO: &str = "2B6523B0-9D08-424B-94CC-6BA173C2DF26";
