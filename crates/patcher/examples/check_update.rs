//! 手工测试 M1（版本检查）：拉真实 CDN 的 `ver2.dat`，与本地版本比较。
//!
//! ```sh
//! # 用临时目录造一个「旧版本」本地元数据，演示「有更新」分支
//! cargo run -p patcher --example check_update -- --make-old
//!
//! # 指定真实安装根（读 <root>/game/LocalVersion3.xml 或 ffxivgame.ver）
//! cargo run -p patcher --example check_update -- --game-dir /path/to/ffxiv
//!
//! # CDN 证书异常时兜底
//! cargo run -p patcher --example check_update -- --make-old --insecure
//! ```
//!
//! 需要打包密钥（见 `build.rs`）：`SDO_FFXIV_META_DES_KEY` / `SDO_FFXIV_CDN_RSA_PUBLIC_KEY`。

use std::path::PathBuf;

fn main() {
    let mut game_dir: Option<PathBuf> = None;
    let mut insecure = false;
    let mut make_old = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--game-dir" => game_dir = args.next().map(PathBuf::from),
            "--insecure" => insecure = true,
            "--make-old" => make_old = true,
            "-h" | "--help" => {
                println!("用法：check_update [--game-dir <安装根>] [--make-old] [--insecure]");
                return;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let cdn = match patcher::cdn::Cdn::with_options(insecure, patcher::cdn::ProxyMode::Env) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // 1) 拉 CDN 最新版本
    let ver2 = match cdn.fetch_ver2(patcher::cdn::GAME_ID, patcher::cdn::BUILD_ID) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("拉取 ver2.dat 失败：{e}");
            std::process::exit(1);
        }
    };
    let remote = match ver2.latest() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("解析 ver2.dat 失败：{e}");
            std::process::exit(1);
        }
    };
    println!(
        "CDN 最新：internal={} display={} name={} packages={}",
        remote.internal,
        remote.display,
        remote.name,
        ver2.packages.len()
    );

    // 2) 准备本地目录
    let mut tmp_guard = None;
    let root = match game_dir {
        Some(p) => p,
        None => {
            let dir = std::env::temp_dir().join(format!("patcher-example-{}", std::process::id()));
            let game = dir.join("game");
            std::fs::create_dir_all(&game).expect("建临时目录");
            if make_old {
                // 用一个比 CDN 旧的版本写加密元数据，走「有更新」分支。
                let meta = patcher::VersionMeta::new(
                    patcher::cdn::GAME_ID,
                    patcher::cdn::BUILD_ID,
                    "0.0.0.26",
                    "2026.09.01.0000.0000_7.56",
                );
                std::fs::write(game.join("LocalVersion3.xml"), patcher::version_meta::build(&meta))
                    .expect("写本地元数据");
            }
            tmp_guard = Some(dir.clone());
            dir
        }
    };
    println!("本地目录：{}", root.display());

    // 3) 比较
    match patcher::check_update_with(&root, &cdn, patcher::cdn::GAME_ID, patcher::cdn::BUILD_ID) {
        Ok(outcome) => {
            match &outcome.local {
                Some(l) => println!(
                    "本地版本：display={} internal={:?} 来源={:?}",
                    l.display, l.internal, l.source
                ),
                None => println!("本地版本：无（全新安装）"),
            }
            println!("结论：{:?}", outcome.decision);
        }
        Err(e) => {
            eprintln!("版本检查失败：{e}");
            if let Some(d) = tmp_guard {
                let _ = std::fs::remove_dir_all(d);
            }
            std::process::exit(1);
        }
    }

    if let Some(d) = tmp_guard {
        let _ = std::fs::remove_dir_all(d);
    }
}
