//! 附属请求：全部后台发送，失败不阻断（人脸验证除外，见 [`Client::check_face_verify`]）。
//! `tgt0` 系登录应用。
//!
//! 后台请求的句柄登记在发起它的 [`Client`] 上（私有 `Pending`），不是进程级全局状态；
//! 退出前必须调用 [`Client::wait_pending`] 等待，否则 `process::exit` 会直接终止这些线程。

use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use proto::log;

use crate::client::{Client, Result, fetch};
use crate::endpoint::{
    Agreement, Endpoint, FaceVerify, LoginUserInfo, Promotion, Suffix, SystemConfig,
};
use crate::error::Error;
use crate::resp;

/// 一个 [`Client`] 发出的后台附属请求句柄。
///
/// 克隆 `Client` 共享同一份清单；[`Pending::wait`] 取走并等待，可重复调用。
#[derive(Debug, Clone, Default)]
pub(crate) struct Pending(Arc<Mutex<Vec<JoinHandle<()>>>>);

impl Pending {
    pub(crate) fn push(&self, handle: JoinHandle<()>) {
        match self.0.lock() {
            Ok(mut v) => v.push(handle),
            // 取锁失败时不再登记，请求照常发出。
            Err(_) => log::debug("附属请求未能登记句柄"),
        }
    }

    /// 等待所有未完成请求结束，总预算 `budget`；超时未完成的只写 WARN 后放弃。
    pub(crate) fn wait(&self, budget: Duration) {
        let handles: Vec<JoinHandle<()>> = match self.0.lock() {
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
}

impl Client {
    fn aux_suffix(&self) -> Suffix {
        Suffix::login(self.identity(), self.run_time_id(), self.app())
    }

    fn aux_no_group_suffix(&self) -> Suffix {
        Suffix::login_no_group(self.identity(), self.run_time_id(), self.app())
    }

    /// 登录前附属请求，先于拿 guid 发出。
    pub fn pre_login(&self) {
        let app = self.app();
        self.spawn_aux("agreement", Agreement::new(app.app_id));
    }

    /// 登录后附属请求中可后台发出的部分（`promotion/userInfo/systemConfig`）。
    pub fn post_login_fire_and_forget(&self, tgt0: &str) {
        let s = self.aux_suffix();
        self.spawn_aux("getPromotionInfo", Promotion::new(s.clone(), tgt0));
        self.spawn_aux("getLoginUserInfo", LoginUserInfo::new(s, tgt0));
        self.spawn_aux(
            "getSystemConfig",
            SystemConfig::new(self.aux_no_group_suffix()),
        );
    }

    /// 人脸验证（同步）：成功 `resultCode == 0 && openFace == "0"`（注意顶层非 `return_code`）；
    /// `openFace == "1"` 终止；其余非成功只记日志，不阻断。
    pub fn check_face_verify(&self, tgt0: &str) -> Result<()> {
        let app = self.app();
        let ep = FaceVerify::new(
            self.identity().device_id.clone(),
            app.app_id,
            app.area_id.clone(),
            app.product_version,
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
            return Err(Error::rejected("需要人脸验证，请在官方客户端完成验证后重试")
                .with_detail("openFace=1"));
        }
        if rc != Some(0) {
            log::debug(&format!(
                "人脸验证接口返回非成功，已忽略：resultCode={rc:?}"
            ));
        }
        Ok(())
    }

    /// 后台请求，失败只写 debug 日志；句柄登记到本客户端，供 [`Client::wait_pending`] 等待。
    fn spawn_aux<E: Endpoint + Send + 'static>(&self, tag: &'static str, ep: E) {
        let handle = std::thread::spawn(move || {
            if let Err(e) = fetch(E::HOST, &ep.path(), E::TIMEOUT) {
                log::debug(&format!("附属请求[{tag}]失败：{}", e.log_text()));
            }
        });
        self.pending.push(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// `Pending` 是每个 `Client` 自己的实例状态：等待会取走并 join 句柄。
    #[test]
    fn pending_joins_and_respects_budget() {
        let done = Arc::new(AtomicBool::new(false));
        let flag = done.clone();
        let pending = Pending::default();
        pending.push(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            flag.store(true, Ordering::SeqCst);
        }));
        pending.wait(Duration::from_secs(2));
        assert!(done.load(Ordering::SeqCst), "wait 应等线程跑完");
        // 句柄已被取走：再等一次是空操作。
        pending.wait(Duration::from_millis(10));

        // 超预算：线程要睡 30s，应当尽快返回。
        pending.push(std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(30))
        }));
        let t0 = Instant::now();
        pending.wait(Duration::from_millis(100));
        assert!(t0.elapsed() < Duration::from_secs(2), "超预算应尽快返回");
    }
}
