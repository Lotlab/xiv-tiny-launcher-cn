#!/usr/bin/env bash
# 运行自检清单并落盘到 docs/SELFCHECK-output.txt（联调留档用；该文件已被 .gitignore 忽略）
#
# 用法：
#   scripts/self-check.sh                                  # 默认工具链的 release 产物
#   TARGET=x86_64-pc-windows-gnu scripts/self-check.sh     # 指定 target 的产物
#   scripts/self-check.sh <exe 路径>                        # 直接指定产物
set -euo pipefail
cd "$(dirname "$0")/.."

HOST_TARGET=$(rustc -vV | awk '/^host:/{print $2}')
TARGET=${TARGET:-$HOST_TARGET}
TRIPLE="$TARGET/"

BIN=${1:-target/${TRIPLE}release/sdo-ffxiv-launcher.exe}
if [ ! -f "$BIN" ]; then
  BIN=target/${TRIPLE}debug/sdo-ffxiv-launcher.exe
fi

echo "使用 $BIN"
mkdir -p docs
"$BIN" --self-check 2>&1 | tee docs/SELFCHECK-output.txt
echo
echo "日志目录：%TEMP%\\SdoFfxiv\\launcher.log（--log-file 可覆盖；%TEMP% 不可写时回退到 EXE 所在目录；脱敏：ticket/tgt/keepLoginKey/guid/codeKey/authorization）"
