//! `--self-check`：自检清单逐项输出（不启动游戏）。

use proto::device::Device;
use proto::enc;
use proto::log;
use sdo_client::{GAME_APP_ID, LOGIN_APP};

use crate::cli::Args;
use crate::game::GameDirs;

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

    let mut api = sdo_client::Api::new(
        sdo_client::Identity::from(device),
        run_time_id.to_owned(),
    );
    let login_app = sdo_client::LOGIN_APP;
    let game_app = sdo_client::App::game(7);

    // QR 探测（不扫码）。
    let qr_probe = probe_qr(&mut api, &login_app);

    // Cookie：只确认请求默认不带 Cookie，需要技术人员抓包复核。
    r.line(
        "Cookie 检查",
        Verdict::Manual,
        "已跳过，需技术人员确认",
    );

    // QR（不扫码）
    match qr_probe {
        Ok((guid, bytes)) => {
            if !guid.is_empty() && bytes > 0 {
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
            log::debug(&format!("自检二维码接口细节：{}", e.log_text()));
            r.line(
                "二维码接口",
                Verdict::Fail,
                &format!("网络异常：{e}"),
            )
        }
    }

    let leg_ok = check_sso_template(&api, &login_app, &game_app);
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
    match crate::areas::fetch_table(&api) {
        Ok(table) => {
            let mut all_ok = true;
            for a in &table.sub_areas {
                match crate::cmdline::Builder::new(a).build() {
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

    // 启动方式：非 Windows 平台得有兼容层才跑得起来。
    match crate::winproc::resolve_launcher(args.run_via.as_deref()) {
        Ok(None) => r.line("启动方式", Verdict::Pass, "直接运行 exe"),
        Ok(Some(l)) => r.line("启动方式", Verdict::Pass, &format!("经兼容层：{l}")),
        Err(e) => r.line("启动方式", Verdict::Fail, &e.replace('\n', " ")),
    }

    // 回退路径
    let fb_ok = !api
        .probe_paths(&login_app, &game_app)
        .fast_in_login
        .contains("groupId");
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

/// 二维码接口探测（不扫码）：取 guid + 一张码；两者都成功即接口可用。
fn probe_qr(api: &mut sdo_client::Api, app: &sdo_client::App) -> sdo_client::Result<(String, usize)> {
    let guid = api.get_guid(app)?;
    let code = api.get_code_key(app)?;
    Ok((guid, code.png.len()))
}

/// 换票两步的参数口径（用样例选区 7 渲染）。
fn check_sso_template(api: &sdo_client::Api, app: &sdo_client::App, target: &sdo_client::App) -> bool {
    let p = api.probe_paths(app, target);
    !p.sso_authorization_login.contains("guid=")
        && !p.sso_authorization_login.contains("tgt=")
        && p.sso_authorization_login.contains("&epIp=&epName=")
        && p.sso_authorization_login.contains("&runTimeId=&channelId=")
        && p.sso_authorization_login.contains(&format!(
            "productVersion={}",
            enc::url_encode(target.product_version)
        ))
        && p.sso_authorization.contains(&format!(
            "productVersion={}",
            enc::url_encode(LOGIN_APP.product_version)
        ))
        && p.sso_authorization.contains("&scene=V3Launcher&")
}
