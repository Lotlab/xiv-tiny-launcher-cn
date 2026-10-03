//! 一次登录会话的共享输入。票据结构随登录链流动。

use std::path::PathBuf;

use proto::device::Device;

use crate::cli::Args;

pub struct Ctx {
    pub device: Device,
    pub device_path: PathBuf,
    pub run_time_id: String,
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

/// 登录链拿到的原始票据。
#[derive(Clone)]
pub struct LoginTicket {
    pub ticket: String,
    pub tgt: String,
    pub guid: String,
}

/// 换票后交给游戏的票据。
#[derive(Clone)]
pub struct GameTicket {
    pub ticket: String,
    pub snda_id: String,
}
