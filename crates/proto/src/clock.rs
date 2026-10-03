//! 当前时间取值（避免为时间戳引入额外依赖）。
//!
//! 取不到系统时间时统一给 0：调用方要么只用它做缓存失效（0 也能用），
//! 要么只是拿它当 PRNG 不可用时的兜底种子。

/// 当前 Unix 毫秒。
pub fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// 当前 Unix 纳秒。
pub(crate) fn now_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_monotonic_enough_for_callers() {
        let a = now_millis();
        let b = now_nanos();
        assert!(a > 1_600_000_000_000, "毫秒取值应晚于 2020 年：{a}");
        assert!(u128::from(b) / 1_000_000 >= a, "纳秒与毫秒应指向同一时刻");
    }
}
