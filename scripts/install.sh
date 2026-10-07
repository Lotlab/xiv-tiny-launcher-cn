#!/usr/bin/env bash
# 安装/更新到游戏目录。
#
# 游戏加载  <exe 所在目录>\..\sdo\sdologin\sdologinentry64.dll
#   —— 即 <游戏根>\sdo\sdologin\sdologinentry64.dll，**不是** game\ 目录；
#      放错位置游戏会报 `5003 账号认证错误`（或加载官方原件后认证失败）。
#
# 用法: scripts/install.sh <游戏根目录，例如 C:\\Games\\FFXIV>
set -euo pipefail
cd "$(dirname "$0")/.."

ROOT=${1:?用法: scripts/install.sh <游戏根目录，例如 C:\\Games\\FFXIV>}
[ -f "$ROOT/game/ffxiv_dx11.exe" ] || {
  echo "不是有效的游戏根目录（缺少 game/ffxiv_dx11.exe）：$ROOT" >&2
  exit 1
}
[ -f dist/launcher.exe ] || {
  echo "缺少 dist/ 产物，先运行 scripts/build.sh" >&2
  exit 1
}

DLLDIR="$ROOT/sdo/sdologin"
mkdir -p "$DLLDIR"

if [ -f "$DLLDIR/sdologinentry64.dll" ] &&
   ! grep -qa "xiv-tiny-launcher-cn/sdologinentry64" "$DLLDIR/sdologinentry64.dll"; then
  echo "备份非自研 DLL → $DLLDIR/sdologinentry64.dll.official.bak"
  cp -f "$DLLDIR/sdologinentry64.dll" "$DLLDIR/sdologinentry64.dll.official.bak"
fi

cp -f dist/sdologinentry64.dll "$DLLDIR/sdologinentry64.dll"
grep -qa "xiv-tiny-launcher-cn/sdologinentry64" "$DLLDIR/sdologinentry64.dll" ||
  { echo "复制后未在目标文件里找到自研构建标记，安装失败" >&2; exit 1; }

cp -f dist/launcher.exe "$ROOT/launcher.exe"
# 瞬时替换的自研副本来源之一（启动器按需取用，不会被游戏加载）：
cp -f dist/sdologinentry64.dll "$ROOT/sdologinentry64.ours.dll"

echo "已安装："
ls -l "$DLLDIR/sdologinentry64.dll" "$ROOT/launcher.exe" "$ROOT/sdologinentry64.ours.dll"
echo
echo "运行：$ROOT\\launcher.exe --self-check   （自检会复核 DLL 落点）"
echo "注意：sdologinsdk64.dll / SdoLoginComServer.exe 不需要安装。"
