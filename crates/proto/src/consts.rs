//! EXE↔DLL 的 ABI 常量，以及 `proto` 自己的模块（`device` / `paths`）用的本地路径。
//!
//! 协议表与流程默认值在 `sdo-client::consts`；EXE 侧的游戏/文件名在 `launcher::consts`；
//! DLL 侧的 IID 表与日志名在 `sdologinentry64::consts`。

// 交接环境变量：EXE 设置，游戏侧 DLL 只读。
pub const ENV_TICKET: &str = "SDO_FFXIV_TICKET";
pub const ENV_SNDAID: &str = "SDO_FFXIV_SNDAID";
pub const ENV_AREAID: &str = "SDO_FFXIV_AREAID";
pub const ENV_BASE: &str = "SDO_FFXIV_BASE";

/// 自研 DLL 的文件名；游戏按 `<exe 目录>\..\sdo\sdologin\<该名>` 动态加载。
pub const DLL_NAME: &str = "sdologinentry64.dll";
/// 自研 DLL 内的构建标记；启动器据此确认目标位置放的是本项目产物而不是官方原件。
pub const DLL_BUILD_MARKER: &str = "xiv-tiny-launcher-cn/sdologinentry64 build-marker v1";

/// 瞬时替换的握手环境变量：启动器设为本次运行的随机 nonce，
/// DLL 在 `SDOLInitialize` 成功后把该值原文写进同目录的 [`SWAP_MARKER_NAME`]。
/// 只在 swap 模式下设置；缺席或为空时 DLL 不写任何文件。
pub const ENV_SWAP: &str = "SDO_FFXIV_SWAP";
/// 握手 marker 文件名（落在游戏 `sdo\sdologin` 目录，Linux/wine 下与启动器共享同一路径）。
pub const SWAP_MARKER_NAME: &str = "sdologinentry64.loaded";

// `proto` 自己的模块用的路径：`device.json` 在 EXE 当前目录，日志在 %TEMP%\SdoFfxiv。
pub const FILE_DEVICE: &str = "device.json";
pub const LOG_DIR_NAME: &str = "SdoFfxiv";
/// 单实例锁文件名（放在每用户私有目录；见 `paths::lock_file`）。
pub const LOCK_FILE_NAME: &str = "SdoFfxiv.lock";
