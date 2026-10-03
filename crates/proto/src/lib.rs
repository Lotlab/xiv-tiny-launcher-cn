//! EXE 与 DLL 共用层（无任何网络代码）：
//! 常量、编码、设备档案、脱敏日志、本地路径、时钟。
//!
//! 网络相关（URL 模板、响应解析、区服表）在 `sdo-client`。

pub mod clock;
pub mod consts;
pub mod device;
pub mod enc;
pub mod log;
pub mod paths;
