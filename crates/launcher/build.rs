//! 构建时把自研登录 DLL 嵌进 exe（瞬时替换的零附带文件来源）。
//!
//! 输入：环境变量 `SDOLOGINENTRY64_DLL`（`scripts/build.sh` 在编完 DLL 后导出绝对路径）。
//! 找不到时退化为 sidecar 模式（`--ours-dll` / 启动器旁的 `sdologinentry64.ours.dll`），
//! 不阻断构建——`dllswap::has_embedded_dll()` 在运行时可查。

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_dll)");
    println!("cargo:rerun-if-env-changed=SDOLOGINENTRY64_DLL");

    let from_env = std::env::var("SDOLOGINENTRY64_DLL")
        .ok()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from);
    // 兜底：工作区 dist 产物（二次构建时通常已存在）。
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("dist")
        .join("sdologinentry64.dll");
    let cand = from_env.or(if fallback.is_file() {
        Some(fallback)
    } else {
        None
    });

    let Some(path) = cand else {
        return;
    };
    if !path.is_file() {
        println!(
            "cargo:warning=SDOLOGINENTRY64_DLL 指向的文件不存在（{}），本次构建不内嵌 DLL",
            path.display()
        );
        return;
    }
    // 构建标记断言：防止嵌错文件（比如官方 DLL）。
    let bytes = std::fs::read(&path).unwrap_or_default();
    if !bytes
        .windows(b"xiv-tiny-launcher-cn/sdologinentry64".len())
        .any(|w| w == b"xiv-tiny-launcher-cn/sdologinentry64")
    {
        panic!(
            "SDOLOGINENTRY64_DLL 不是自研 DLL（缺少构建标记）：{}",
            path.display()
        );
    }
    println!("cargo:rustc-cfg=embedded_dll");
    println!("cargo:rustc-env=EMBEDDED_DLL_PATH={}", path.display());
    println!("cargo:rerun-if-changed={}", path.display());
}
