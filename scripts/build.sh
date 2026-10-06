#!/usr/bin/env bash
# 构建交付物：sdo-ffxiv-launcher.exe + sdologinentry64.dll
#
# 目标不钉死：TARGET 空 = 用本机默认工具链（本仓库默认走 MSVC + 静态 CRT）。
#   scripts/build.sh                                              # 默认（MSVC）
#   TARGET=x86_64-pc-windows-gnu scripts/build.sh                 # GNU（需 MSYS2 提供 gcc）
#   scripts/build.sh x86_64-pc-windows-msvc                       # 也可直接把 target 当第一个参数
#
# 两个 target 的交付约束见 README（不引入 VC++ 运行库）。
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET=${TARGET:-${1:-}}

# 打包密钥（见 crates/patcher/build.rs）：不进版本库，缺失即构建失败。
#   keys/cdn_rsa_public.pem   CDN 鉴权 RSA 公钥（PEM）
#   keys/meta_des.key         本地版本元数据 3DES 密钥（32 位 hex）
# 也可直接用环境变量覆盖。
resolve_key() {
  local env_name=$1 file=$2 what=$3
  if [ -n "${!env_name:-}" ]; then return 0; fi
  if [ -f "$file" ]; then export "$env_name=$(cat "$file")"; return 0; fi
  echo "缺少 $what：请设置 $env_name，或提供 $file（keys/ 已 gitignore）" >&2
  return 1
}
resolve_key SDO_FFXIV_CDN_RSA_PUBLIC_KEY keys/cdn_rsa_public.pem "CDN RSA 公钥"
resolve_key SDO_FFXIV_META_DES_KEY      keys/meta_des.key      "本地元数据 3DES 密钥"

# 显式指定 gnu 时才要求 mingw gcc（链接器由该工具链提供；rustc 对 gnu 目标不自带链接器）。
case "$TARGET" in
  *windows-gnu)
    if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
      echo "缺少 x86_64-w64-mingw32-gcc（ucrt64 工具链提供），无法链接 $TARGET 目标" >&2
      exit 1
    fi
    ;;
esac

CARGO_ARGS=()
if [ -n "$TARGET" ]; then
  CARGO_ARGS=(--target "$TARGET")
fi

echo "=== cargo test --workspace ${CARGO_ARGS[*]:-（默认工具链）} ==="
cargo test --workspace "${CARGO_ARGS[@]}"

echo "=== cargo build --release --workspace ==="
cargo build --release --workspace "${CARGO_ARGS[@]}"

# 产物目录推导：显式 target 用 $TARGET，否则取 rustc 的 host 三元组。
HOST_TARGET=$(rustc -vV | awk '/^host:/{print $2}')
RESOLVED_TARGET=${TARGET:-$HOST_TARGET}
OUT="target/$RESOLVED_TARGET/release"

echo "=== 交付物（target=$RESOLVED_TARGET）==="
for f in sdo-ffxiv-launcher.exe sdologinentry64.dll; do
  [ -f "$OUT/$f" ] || { echo "缺少产物 $OUT/$f" >&2; exit 1; }
done

# 断言：不得依赖 VC++ 运行库（DLL 注入游戏进程，缺依赖会表现为认证失败而非明确报错）。
# 用字符串搜索而非 objdump，避免依赖 binutils 是否在 PATH 及其 PE 输出差异。
for f in sdo-ffxiv-launcher.exe sdologinentry64.dll; do
  if grep -aqi "vcruntime140" "$OUT/$f"; then
    echo "$f 依赖 VCRUNTIME140.dll（VC++ 运行库），干净 Windows 上会加载失败" >&2
    exit 1
  fi
done

mkdir -p dist
cp -f "$OUT/sdo-ffxiv-launcher.exe" dist/
cp -f "$OUT/sdologinentry64.dll" dist/

ls -l dist

echo
echo "安装：scripts/install.sh \"<安装根，例如 C:\\Games\\FFXIV>\""
echo "  启动器   → <安装根>\\sdo-ffxiv-launcher.exe"
echo "  登录 DLL → <安装根>\\sdo\\sdologin\\sdologinentry64.dll"
