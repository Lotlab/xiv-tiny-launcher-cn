//! 打包时注入密钥。
//!
//! 两样东西都不进版本库，缺任何一个都让编译失败（避免悄悄产出不可用/用错密钥的产物）：
//!
//! - `SDO_FFXIV_CDN_RSA_PUBLIC_KEY`：CDN `v3ctrl.xml` 鉴权用的 RSA **公钥**（PEM）。
//!   来源优先级：env 全文 → `SDO_FFXIV_CDN_RSA_PUBLIC_KEY_FILE` → `keys/cdn_rsa_public.pem`。
//! - `SDO_FFXIV_META_DES_KEY`：本地版本元数据 `LocalVersion3.xml` 的 3DES 密钥（16 字节 hex）。
//!   来源优先级：env → `SDO_FFXIV_META_DES_KEY_FILE` → `keys/meta_des.key`。
//!
//! 注入后由 `src/keys.rs` 用 `env!` 读回；源码里不出现明文。

use std::path::PathBuf;

/// 密钥目录：build script 的 CWD 是**包根**（`crates/patcher/`），密钥放仓库根的 `keys/`。
const KEYS_DIR: &str = "../../keys";

fn key_file(name: &str) -> PathBuf {
    PathBuf::from(KEYS_DIR).join(name)
}

fn main() {
    for v in [
        "SDO_FFXIV_CDN_RSA_PUBLIC_KEY",
        "SDO_FFXIV_CDN_RSA_PUBLIC_KEY_FILE",
        "SDO_FFXIV_META_DES_KEY",
        "SDO_FFXIV_META_DES_KEY_FILE",
    ] {
        println!("cargo:rerun-if-env-changed={v}");
    }
    for f in ["cdn_rsa_public.pem", "meta_des.key"] {
        println!("cargo:rerun-if-changed={}", key_file(f).display());
    }

    let rsa = read_rsa();
    let des = read_des();

    // PEM 是多行的，`cargo:rustc-env` 只能带一行；写进 OUT_DIR 再由 `include_str!` 读回。
    let out_dir = std::env::var("OUT_DIR").expect("cargo 应提供 OUT_DIR");
    let pem_path = std::path::Path::new(&out_dir).join("cdn_rsa_public.pem");
    std::fs::write(&pem_path, rsa.as_bytes()).expect("写 RSA 公钥到 OUT_DIR 失败");
    println!(
        "cargo:rustc-env=SDO_FFXIV_CDN_RSA_PUBLIC_KEY_PEM_FILE={}",
        pem_path.display()
    );
    println!("cargo:rustc-env=SDO_FFXIV_META_DES_KEY_HEX={des}");
}

fn read_rsa() -> String {
    if let Ok(v) = std::env::var("SDO_FFXIV_CDN_RSA_PUBLIC_KEY") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    let file = std::env::var("SDO_FFXIV_CDN_RSA_PUBLIC_KEY_FILE")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_file("cdn_rsa_public.pem"));
    match std::fs::read_to_string(&file) {
        Ok(s) if !s.trim().is_empty() => s.trim().to_string(),
        _ => fail(
            "CDN RSA 公钥",
            "SDO_FFXIV_CDN_RSA_PUBLIC_KEY（PEM 全文）/ SDO_FFXIV_CDN_RSA_PUBLIC_KEY_FILE / keys/cdn_rsa_public.pem",
        ),
    }
}

fn read_des() -> String {
    let raw = if let Ok(v) = std::env::var("SDO_FFXIV_META_DES_KEY") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            v
        } else {
            read_des_file()
        }
    } else {
        read_des_file()
    };
    // 允许带分隔符，统一成 32 位小写 hex。
    let hex: String = raw
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .collect::<String>()
        .to_ascii_lowercase();
    if hex.len() != 32 {
        fail(
            &format!("本地元数据 3DES 密钥（应为 16 字节 = 32 位 hex，实际 {} 位）", hex.len()),
            "SDO_FFXIV_META_DES_KEY（32 位 hex）/ SDO_FFXIV_META_DES_KEY_FILE / keys/meta_des.key",
        );
    }
    hex
}

fn read_des_file() -> String {
    let file = std::env::var("SDO_FFXIV_META_DES_KEY_FILE")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| key_file("meta_des.key"));
    std::fs::read_to_string(&file).unwrap_or_default()
}

fn fail(what: &str, how: &str) -> ! {
    panic!(
        "\n\n缺少 {what}。打包时必须提供（不进版本库）：\n  {how}\n\
         也可把文件放到仓库根的 keys/ 下（该目录已 gitignore）。\n"
    )
}
