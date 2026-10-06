//! 手工测试 M3 的「补丁链 + 补丁清单」部分（只拉小 JSON，不下载大 zip）。
//!
//! ```sh
//! cargo run -p patcher --example incremental_info -- --from 0.0.0.26
//! ```
//!
//! 需要打包密钥（见 `build.rs`）。

use patcher::cdn::{self, Cdn};
use patcher::download::Downloader;
use patcher::patch::{self, PatchFileList};

/// 这个例子不需要进度回调。
struct Silent;
impl patcher::download::Progress for Silent {}

fn main() {
    let mut from = String::from("0.0.0.26");
    let mut insecure = false;
    let mut extract = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--from" => from = args.next().unwrap_or(from),
            "--insecure" => insecure = true,
            "--extract" => extract = true,
            "-h" | "--help" => {
                println!("用法：incremental_info [--from <internal>] [--insecure] [--extract]");
                return;
            }
            other => {
                eprintln!("未知参数：{other}");
                std::process::exit(2);
            }
        }
    }

    let cdn = Cdn::with_options(insecure, cdn::ProxyMode::Env).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let ver2 = cdn.fetch_ver2(cdn::GAME_ID, cdn::BUILD_ID).unwrap_or_else(|e| {
        eprintln!("拉 ver2.dat 失败：{e}");
        std::process::exit(1);
    });
    let remote = ver2.latest().unwrap();
    println!(
        "CDN 最新：{}（{}）display={}",
        remote.internal, remote.name, remote.display
    );

    let chain = match patch::patch_chain(&ver2, &from, "（本地 display）", &remote.internal, 32) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("补丁链失败：{e}");
            std::process::exit(1);
        }
    };
    println!("补丁链：{} → {}，共 {} 跳", from, remote.internal, chain.len());

    let auth = cdn.fetch_auth(cdn::GAME_ID).unwrap();
    let mut dl = Downloader::new(&cdn, auth, cdn::GAME_ID, 3);

    let mut total = 0u64;
    for (i, hop) in chain.iter().enumerate() {
        let url = format!("{}{}", hop.base_url, hop.file_list_url);
        println!("\n[{}/{}] {} → {}", i + 1, chain.len(), hop.from, hop.to);
        println!("  清单 URL={}", url);
        let bytes = match dl.fetch_authed_bytes(&url) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("  拉补丁清单失败：{e}");
                std::process::exit(1);
            }
        };
        let pfl: PatchFileList = match serde_json::from_slice(&bytes) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("  解析补丁清单失败：{e}");
                std::process::exit(1);
            }
        };
        println!("  zip 目录={}", pfl.base_url);
        let hop_bytes: u64 = pfl.files.iter().map(|z| z.size).sum();
        total += hop_bytes;
        for z in &pfl.files {
            println!(
                "    {}  {:.1} MB  md5={}",
                PatchFileList::zip_name(z).unwrap_or_else(|| z.url.clone()),
                z.size as f64 / 1e6,
                z.md5
            );
        }
        println!("  本跳合计 {:.2} GB", hop_bytes as f64 / 1e9);

        if extract {
            let work = std::env::temp_dir().join(format!("patcher-extract-{}", std::process::id()));
            let zip_dir = work.join("zips");
            let delta_dir = work.join("deltas");
            let mut zips = Vec::new();
            for z in &pfl.files {
                let name = PatchFileList::zip_name(z).unwrap_or_else(|| {
                    eprintln!("补丁包路径非法：{}", z.url);
                    std::process::exit(1);
                });
                let dest = zip_dir.join(&name);
                print!("  下载 {name} … ");
                use std::io::Write;
                std::io::stdout().flush().ok();
                dl.fetch_url(&pfl.zip_url(z), &dest, z.size, &z.md5, &name, &mut Silent)
                    .unwrap_or_else(|e| {
                        eprintln!("\n下载失败：{e}");
                        std::process::exit(1);
                    });
                println!("完成");
                zips.push(dest);
            }
            print!("  解出 delta … ");
            use std::io::Write;
            std::io::stdout().flush().ok();
            let entries = patch::extract_deltas(&zips, &delta_dir).unwrap_or_else(|e| {
                eprintln!("\n解出失败：{e}");
                std::process::exit(1);
            });
            println!("{} 条", entries.len());
            for e in entries.iter().take(5) {
                println!(
                    "    {} origin={} result={}",
                    e.rel_path, e.origin_md5, e.result_md5
                );
            }
            if entries.len() > 5 {
                println!("    … 其余 {} 条", entries.len() - 5);
            }
            let _ = std::fs::remove_dir_all(&work);
        }
    }
    println!("\n全部补丁包合计 {:.2} GB", total as f64 / 1e9);
}
