//! 手工测试 M2（鉴权 + 单文件下载）：拉 `v3ctrl.xml` → 解鉴权 → 按清单下一个小文件。
//!
//! ```sh
//! # 默认下 game\ffxivgame.ver（很小）到临时目录
//! cargo run -p patcher --example download_one
//!
//! # 指定清单里的路径（反斜杠原样）与输出目录
//! cargo run -p patcher --example download_one -- --path "game\\ffxivgame.ver" --out /tmp/out
//! ```
//!
//! 需要打包密钥（见 `build.rs`）。

use std::path::PathBuf;

fn main() {
    let mut want = String::from("game\\ffxivgame.ver");
    let mut out: Option<PathBuf> = None;
    let mut insecure = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--path" => want = args.next().unwrap_or(want),
            "--out" => out = args.next().map(PathBuf::from),
            "--insecure" => insecure = true,
            "-h" | "--help" => {
                println!("用法：download_one [--path <清单路径>] [--out <目录>] [--insecure]");
                return;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let cdn = patcher::cdn::Cdn::with_options(insecure, patcher::cdn::ProxyMode::Env).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    // 1) 鉴权：v3ctrl.xml + RSA 公钥解密
    let auth = match cdn.fetch_auth(patcher::cdn::GAME_ID) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("拉取/解析 v3ctrl.xml 失败：{e}");
            std::process::exit(1);
        }
    };
    println!(
        "鉴权：flag={} Referer={} UA={} token={}",
        auth.config_flag,
        auth.project_md5_key,
        auth.referer_value,
        auth.cdn_token
    );

    // 2) 文件清单
    let list = match cdn.fetch_file_list(patcher::cdn::GAME_ID, patcher::cdn::BUILD_ID) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("拉取文件清单失败：{e}");
            std::process::exit(1);
        }
    };
    println!(
        "清单：{} 个文件，共 {:.2} GB，base={}",
        list.files.len(),
        list.files.iter().map(|f| f.size).sum::<u64>() as f64 / 1e9,
        list.hash_base_path
    );

    // 3) 找到目标条目
    let entry = list
        .files
        .iter()
        .find(|f| f.path.eq_ignore_ascii_case(&want))
        .unwrap_or_else(|| {
            eprintln!("清单里没有 {want}");
            std::process::exit(1);
        })
        .clone();
    let url = list.url_for(&entry);
    println!(
        "目标：{} size={} md5={}\n  URL={}",
        entry.path, entry.size, entry.hash, url
    );
    println!("  鉴权后 URL={}", auth.auth_url(&url));

    // 4) 下载
    let root = out.unwrap_or_else(|| {
        std::env::temp_dir().join(format!("patcher-dl-{}", std::process::id()))
    });
    let dest = list.local_path(&root, &entry);
    println!("落盘：{}", dest.display());

    let mut last = 0u64;
    let mut on_progress = |done: u64| {
        if done / (256 * 1024) != last / (256 * 1024) {
            println!("  …{done}/{}", entry.size);
        }
        last = done;
    };
    match cdn.download_file(&auth, &url, &dest, entry.size, &entry.hash, &mut on_progress) {
        Ok(bytes) => {
            println!("下载完成：{bytes} 字节");
            println!(
                "本地校验：{}",
                if patcher::cdn::verify_local(&dest, &entry) {
                    "PASS"
                } else {
                    "FAIL"
                }
            );
            if let Ok(text) = std::fs::read_to_string(&dest) {
                println!("内容：{:?}", text.trim());
            }
        }
        Err(e) => {
            eprintln!("下载失败：{e}");
            std::process::exit(1);
        }
    }
}
