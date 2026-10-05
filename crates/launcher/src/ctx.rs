//! 一次登录会话的共享输入。票据类型由 `sdo-client` 提供并在此重导出。
//!
//! `runTimeId` 不在其中：它是网络身份的一部分，只属于 `sdo_client::Client`。

use std::path::PathBuf;

use proto::device::Device;

use crate::cli::Args;

pub use sdo_client::LoginTicket;

pub struct Ctx {
    pub device: Device,
    pub device_path: PathBuf,
    pub args: Args,
}

impl Ctx {
    pub fn poll_delay_ms(&self) -> u64 {
        let lo = self.args.poll_min_ms;
        let hi = self.args.poll_max_ms;
        if hi <= lo {
            return lo;
        }
        lo + (proto::enc::random_u64() % (hi - lo + 1))
    }
}
