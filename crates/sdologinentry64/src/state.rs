//! DLL 侧交接读写：只碰进程内的环境变量（唯一数据源，不做缓存）。
//!
//! 除 env 读写外没有任何共享可变状态 —— 游戏可能从多个线程调虚表，
//! 而票据由 EXE 通过 env 一次性交进来，DLL 不需要自己再存一份。

#[cfg(test)]
use std::sync::Mutex;

use proto::consts::{ENV_AREAID, ENV_BASE, ENV_SNDAID, ENV_TICKET};

use crate::win;

/// 交接数据是否就绪（四个 env 齐备）。
pub fn delivery_ready() -> bool {
    win::get_env(ENV_TICKET).is_some()
        && win::get_env(ENV_SNDAID).is_some()
        && win::get_env(ENV_BASE).is_some()
        && win::get_env(ENV_AREAID).is_some()
}

/// `Logout`：清本进程交接 env（不动 `device.json` 的 keepLoginKey）。
pub fn clear_delivery_env() {
    win::clear_env(ENV_TICKET);
    win::clear_env(ENV_SNDAID);
    win::clear_env(ENV_AREAID);
    win::clear_env(ENV_BASE);
}

/// 单元测试串行锁：env 是进程级全局状态，测试并行会互相踩。
#[cfg(test)]
pub static ENV_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_ready_reflects_env() {
        let _g = ENV_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // 测试进程自身环境通常没有交接变量
        clear_delivery_env();
        assert!(!delivery_ready());
        std::env::set_var(ENV_TICKET, "ULS21-TEST");
        std::env::set_var(ENV_SNDAID, "1234567890");
        std::env::set_var(ENV_BASE, "-AppID=100001900 -AreaID=7");
        std::env::set_var(ENV_AREAID, "7");
        assert!(delivery_ready());
        clear_delivery_env();
        assert!(!delivery_ready());
    }
}
