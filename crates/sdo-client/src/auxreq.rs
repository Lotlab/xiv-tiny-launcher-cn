//! 附属请求：全部后台发送，失败不阻断（人脸验证除外，见 [`Client::check_face_verify`]）。
//! `tgt0` 系登录应用。
//!
//! 后台请求的线程句柄登记在进程级清单里；退出前必须调用 [`wait_pending`] 等待，
//! 否则 `process::exit` 会直接终止这些线程。

use std::sync::Mutex;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use proto::log;

use crate::client::{Client, Result, fetch};
use crate::endpoint::{
    Agreement, Endpoint, FaceVerify, LoginUserInfo, Promotion, Suffix, SystemConfig,
};
use crate::error::Error;
use crate::resp;

impl Client {
    fn aux_suffix(&self) -> Suffix {
        Suffix::login(self.identity(), self.run_time_id(), self.login_app())
    }

    fn aux_no_group_suffix(&self) -> Suffix {
        Suffix::login_no_group(self.identity(), self.run_time_id(), self.login_app())
    }

    /// 登录前附属请求，先于拿 guid 发出。
    pub fn pre_login(&self) {
        let app = self.login_app();
        spawn("agreement", Agreement::new(app.app_id.clone()));
    }

    /// 登录后附属请求中可后台发出的部分（`promotion/userInfo/systemConfig`）。
    pub fn post_login_fire_and_forget(&self, tgt0: &str) {
        let s = self.aux_suffix();
        spawn("getPromotionInfo", Promotion::new(s.clone(), tgt0));
        spawn("getLoginUserInfo", LoginUserInfo::new(s, tgt0));
        spawn(
            "getSystemConfig",
            SystemConfig::new(self.aux_no_group_suffix()),
        );
    }

    /// 人脸验证（同步）：成功 `resultCode == 0 && openFace == "0"`（注意顶层非 `return_code`）；
    /// `openFace == "1"` 终止；其余非成功只记日志，不阻断。
    pub fn check_face_verify(&self, tgt0: &str) -> Result<()> {
        let app = self.login_app();
        let ep = FaceVerify::new(
            self.identity().device_id.clone(),
            app.app_id.clone(),
            app.area.clone(),
            app.product_version.clone(),
            tgt0,
        );
        let r = self.get(&ep)?;
        if r.status != 200 {
            return Ok(());
        }
        let json = match r.json() {
            Ok(j) => j,
            Err(_) => {
                return Ok(());
            }
        };
        let rc = resp::result_code(&json);
        let open_face = resp::open_face(&json).unwrap_or_default();
        if open_face == "1" {
            log::warn("需要人脸验证，请在官方客户端完成验证后重试");
            return Err(Error::rejected(
                "需要人脸验证，请在官方客户端完成验证后重试",
            ));
        }
        if rc != Some(0) {
            log::debug("人脸验证接口返回非成功，已忽略");
        }
        Ok(())
    }
}

/// 未完成的后台附属请求句柄。
static PENDING: Mutex<Vec<JoinHandle<()>>> = Mutex::new(Vec::new());

/// 后台请求，失败只写 debug 日志；句柄登记以便退出前等待。
fn spawn<E: Endpoint + Send + 'static>(tag: &'static str, ep: E) {
    let handle = std::thread::spawn(move || {
        if let Err(e) = fetch(E::HOST, &ep.path(), E::TIMEOUT) {
            log::debug(&format!("附属请求[{tag}]失败：{e}"));
        }
    });
    match PENDING.lock() {
        Ok(mut v) => v.push(handle),
        // 取锁失败时不再登记，请求照常发出。
        Err(_) => log::debug(&format!("附属请求[{tag}]未能登记句柄")),
    }
}

/// 等待所有后台附属请求结束，总预算 `budget`；超时未完成的只写 WARN 后放弃。
pub fn wait_pending(budget: Duration) {
    let handles: Vec<JoinHandle<()>> = match PENDING.lock() {
        Ok(mut v) => std::mem::take(&mut *v),
        Err(e) => std::mem::take(&mut *e.into_inner()),
    };
    if handles.is_empty() {
        return;
    }
    let deadline = Instant::now() + budget;
    let mut unfinished = 0usize;
    for h in handles {
        // 标准库没有带超时的 join，只能轮询 is_finished。
        while !h.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        } else {
            unfinished += 1; // 句柄在此丢弃，不再等待。
        }
    }
    if unfinished > 0 {
        log::warn(&format!("附属请求仍有 {unfinished} 个未完成，已放弃等待"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// `PENDING` 是进程级清单，测试必须串行。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// `wait_pending` 会等待已登记的线程；超预算则尽快返回。
    #[test]
    fn wait_pending_joins_and_respects_budget() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        PENDING.lock().unwrap().push(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        wait_pending(Duration::from_secs(2));
        assert!(done.load(Ordering::SeqCst), "wait_pending 应等线程跑完");
        assert!(PENDING.lock().unwrap().is_empty(), "句柄应被取走");

        // 超预算：线程要睡 30s，应当尽快返回。
        PENDING
            .lock()
            .unwrap()
            .push(std::thread::spawn(|| std::thread::sleep(Duration::from_secs(30))));
        let t0 = Instant::now();
        wait_pending(Duration::from_millis(100));
        assert!(t0.elapsed() < Duration::from_secs(2), "超预算应尽快返回");
    }
}
