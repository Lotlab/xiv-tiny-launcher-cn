//! 单实例：后启动的实例直接退出。
//!
//! 用 `std::fs::File::try_lock`（Unix `flock` / Windows `LockFileEx`）：锁由内核持有，
//! 进程退出（含崩溃）即自动释放，不会留下需要手工清理的死锁。
//! 作用域是"每用户一个实例"（锁文件在每用户私有目录里），不是跨用户的全局互斥。

use std::fs::{File, TryLockError};

use proto::paths;

pub struct InstanceGuard {
    // 只用来持有锁；句柄一关（drop）锁就释放，无需显式 unlock。
    _file: File,
}

pub enum Acquire {
    /// 拿到锁，本次是本机唯一（每用户）的实例。
    Guard(InstanceGuard),
    AlreadyRunning,
    /// 创建/加锁失败（IO 错误，含路径不可写）。
    Failed(std::io::Error),
}

pub fn acquire() -> Acquire {
    let path = paths::lock_file();
    let file = match File::create(&path) {
        Ok(f) => f,
        Err(e) => return Acquire::Failed(e),
    };
    match file.try_lock() {
        Ok(()) => Acquire::Guard(InstanceGuard { _file: file }),
        Err(TryLockError::WouldBlock) => Acquire::AlreadyRunning,
        Err(TryLockError::Error(e)) => Acquire::Failed(e),
    }
}
