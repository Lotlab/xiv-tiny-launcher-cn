//! `--self-check`：自检清单逐项输出（不启动游戏）。

use proto::consts::*;
use proto::device::Device;
use proto::enc;
use proto::log;
use proto::params::Suffix;
use proto::server;

use crate::cli::Args;
use crate::ctx::Ctx;
use crate::game::GameDirs;
use crate::{areas, login};

/// 逐项判定。
enum Verdict {
    Pass,
    Fail,
    /// 需要真实登录 / 游戏大厅才能测。
    Skip,
    /// 只能人工核对。
    Manual,
}

impl Verdict {
    fn tag(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Skip => "SKIP(需实测)",
            Verdict::Manual => "MANUAL(需人工)",
        }
    }
}

struct Report {
    pass: u32,
    fail: u32,
    skip: u32,
    manual: u32,
}

impl Report {
    fn new() -> Report {
        Report {
            pass: 0,
            fail: 0,
            skip: 0,
            manual: 0,
        }
    }

    fn count(&self) -> u32 {
        self.pass + self.fail + self.skip + self.manual
    }

    fn line(&mut self, name: &str, verdict: Verdict, detail: &str) {
        match verdict {
            Verdict::Pass => self.pass += 1,
            Verdict::Fail => self.fail += 1,
            Verdict::Skip => self.skip += 1,
            Verdict::Manual => self.manual += 1,
        }
        println!("[{:>2}] {name:<28} {}  {detail}", self.count(), verdict.tag());
    }
}

pub fn run(
    args: &Args,
    device: &Device,
    run_time_id: &str,
    game: Option<GameDirs>,
) -> Result<(), String> {
    let mut r = Report::new();
    println!("\n===== 自检清单（--self-check）=====\n");

    // 设备一致性
    let seg_ok = device.seg0_consistent();
    let fields_ok = proto::device::is_valid_mac(&device.mac_id)
        && device.ep_name.is_ascii()
        && !device.ep_name.is_empty()
        && device.ep_ip.parse::<std::net::Ipv4Addr>().is_ok();
    let ok1 = seg_ok && fields_ok;
    r.line(
        "设备一致性",
        if ok1 { Verdict::Pass } else { Verdict::Fail },
        if ok1 {
            "设备信息合法"
        } else if !seg_ok {
            "设备档案已损坏，请删除 device.json 后重试"
        } else {
            "设备信息存在非法取值，档案已损坏"
        },
    );

    let ctx = Ctx {
        device: device.clone(),
        device_path: proto::paths::cwd_file(FILE_DEVICE),
        run_time_id: run_time_id.to_string(),
        args: args.clone(),
    };

    // QR 探测（不扫码）。
    let qr_probe = login::probe_qr(&ctx);

    // Cookie：只确认请求默认不带 Cookie，需要技术人员抓包复核。
    r.line(
        "Cookie 检查",
        Verdict::Manual,
        "已跳过，需技术人员确认",
    );

    // QR（不扫码）
    match qr_probe {
        Ok(probe) => {
            if !probe.guid.is_empty() && probe.has_codekey_png {
                r.line(
                    "二维码接口",
                    Verdict::Pass,
                    "已取到二维码",
                );
            } else {
                r.line(
                    "二维码接口",
                    Verdict::Fail,
                    "二维码响应异常",
                );
            }
        }
        Err(e) => {
            log::debug(&format!("自检二维码接口细节：{e}"));
            r.line(
                "二维码接口",
                Verdict::Fail,
                &format!("网络异常：{e}"),
            )
        }
    }

    let leg_ok = check_sso_template(device, run_time_id);
    r.line(
        "换票参数模板",
        if leg_ok { Verdict::Pass } else { Verdict::Fail },
        if leg_ok {
            "换票参数模板正确；真实换票需登录后验证"
        } else {
            "换票参数模板异常"
        },
    );

    r.line(
        "登录票据检查",
        Verdict::Manual,
        "需登录游戏后由技术人员确认",
    );

    // 命令行：在线/本地区服表 + 逐子区 base 断言
    match areas::fetch_table() {
        Ok(table) => {
            let mut all_ok = true;
            for a in &table.sub_areas {
                match server::build_base(a) {
                    Ok(base) => {
                        let ok = base.contains(&format!("-AreaID={} ", a.id))
                            && base.starts_with(&format!("-AppID={GAME_APP_ID} "))
                            && base.ends_with("DEV.MaxEntitledExpansionID=1");
                        all_ok &= ok;
                    }
                    Err(_) => {
                        all_ok = false;
                    }
                }
            }
            r.line(
                "启动参数模板",
                if all_ok { Verdict::Pass } else { Verdict::Fail },
                if all_ok {
                    "启动参数模板正确"
                } else {
                    "启动参数模板异常"
                },
            );
        }
        Err(e) => r.line(
            "命令行 base 模板",
            Verdict::Fail,
            &format!("区服表不可用：{e}"),
        ),
    }

    match &game {
        Some(g) => {
            let dll = crate::game::check_login_dll(&g.game_dir);
            let ok = dll.present && dll.is_ours;
            r.line(
                "启动(关键)",
                if ok { Verdict::Skip } else { Verdict::Fail },
                if ok {
                    "登录组件位置正确；能否进大厅需实测"
                } else {
                    "登录组件缺失或不对，请检查安装"
                },
            );
        }
        None => r.line(
            "启动(关键)",
            Verdict::Skip,
            "未找到游戏（用 --game-dir 指定游戏目录），跳过启动检查",
        ),
    }

    // 回退路径
    let fast_suffix = Suffix::login_no_group(device, run_time_id);
    let fast_path = proto::params::path_fast_in_login(&fast_suffix, "<keepLoginKey>");
    let fb_ok = !fast_path.contains("groupId");
    r.line(
        "回退路径",
        if fb_ok { Verdict::Pass } else { Verdict::Fail },
        if fb_ok {
            "自动登录模板正确"
        } else {
            "自动登录模板异常"
        },
    );

    // 登出：DLL 侧行为由单测覆盖，本工具不加载 DLL。
    r.line(
        "登出清理",
        Verdict::Manual,
        "需登录游戏后确认：退出后登录信息已清理",
    );

    r.line(
        "兼容性检查",
        Verdict::Manual,
        "需登录游戏后确认",
    );

    r.line(
        "启动检查",
        Verdict::Manual,
        "需在游戏启动后确认",
    );

    println!(
        "\n汇总（共 {} 项）：PASS {} / FAIL {} / SKIP(需实测) {} / MANUAL(需人工) {}",
        r.count(),
        r.pass,
        r.fail,
        r.skip,
        r.manual
    );
    println!("关键项提醒：登录与启动检查需在真实登录后确认。");
    if r.fail > 0 {
        return Err(format!("自检存在 {} 项 FAIL", r.fail));
    }
    Ok(())
}

fn check_sso_template(device: &Device, run_time_id: &str) -> bool {
    let s1 = Suffix::for_sso_authorization(device, run_time_id, "7");
    let p1 = proto::params::path_get_sso_authorization(&s1, "<tgt0>", "<guid0>");
    let s2 = Suffix::for_sso_login(device, run_time_id, "7");
    let p2 = proto::params::path_sso_authorization_login(&s2, "<UA>");
    !p2.contains("guid=")
        && !p2.contains("tgt=")
        && p2.contains("&epIp=&epName=")
        && p2.contains("&runTimeId=&channelId=")
        && p2.contains(&format!(
            "productVersion={}",
            enc::url_encode(SSO_LOGIN_PRODUCT_VERSION)
        ))
        && p1.contains(&format!(
            "productVersion={}",
            enc::url_encode(SSO_AUTHORIZATION_PRODUCT_VERSION)
        ))
        && p1.contains("&scene=V3Launcher&")
}
