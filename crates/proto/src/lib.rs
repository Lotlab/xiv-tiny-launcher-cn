//! EXE 与 DLL 共用层（无任何网络代码）：
//! 常量、URL 模板、编码、设备档案、区服解析、响应解析、脱敏日志。
//!

pub mod clock;
pub mod consts;
pub mod device;
pub mod enc;
pub mod log;
pub mod params;
pub mod paths;
pub mod resp;
pub mod server;
