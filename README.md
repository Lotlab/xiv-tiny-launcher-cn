# xiv-tiny-launcher-cn

FFXIV 国服的轻量启动器，支持扫码/一键登录、更新等功能。尽量精简外部依赖以便模拟器等平台使用。

## 安装

```bash
scripts/install.sh "C:\Games\FFXIV"
```

装到 `<安装根>\launcher.exe`（与 `game/` 同级）+ `<安装根>\sdo\sdologin\sdologinentry64.dll`
（注意不是 `game\`，放错会报 `5003`）。官方 DLL 自动备份为 `.official.bak`。

## 用法

在安装根下运行：

```bash
launcher.exe                 # 自动：能免扫码就免扫码，否则扫码
launcher.exe --mode qr       # 强制扫码
launcher.exe --mode push --account you@example.com
launcher.exe --area 7        # 指定大区；缺省用上次的，否则进菜单
```

扫码：默认弹窗 + 终端二选一，扫不上就扫当前目录的 `qrcode.png`。按键：`Ctrl+C` 换码、`q` 退出。完整参数见 `launcher.exe --help`。

说明：登录凭据和上次选区记在 `device.json`（删掉即重走扫码）；日志在 `%TEMP%\SdoFfxiv\`，
已脱敏。报 `5003` 且没有 `sdologinentry.log` 时，先检查上面 DLL 落点。

## 从源码构建（维护者）

需求：Rust 稳定版；默认用 MSVC + 静态 CRT，可选 GNU 工具链。

```bash
scripts/build.sh                                  # 测试 + release 构建，产物在 dist/
TARGET=x86_64-pc-windows-gnu scripts/build.sh     # 改用 GNU 工具链
scripts/install.sh "C:\Games\FFXIV"               # 安装到游戏目录（自动备份官方 DLL）
scripts/self-check.sh                             # 自检，输出留档 docs/SELFCHECK-output.txt
```

## 项目结构

```text
crates/
  proto/            # EXE/DLL 共用基础：常量、编码、设备档案、脱敏日志、本地路径
  sdo-client/       # 登录/换票/区服/附属请求的网络层（EXE 侧唯一的网络发起方）
  patcher/          # 游戏版本检查与更新（增量/全量、CDN 下载、差分）
  launcher/         # 启动器本体：CLI、登录编排、二维码渲染、进程拉起、自检
  sdologinentry64/  # 游戏侧 DLL：三导出 + Login/Info 虚表，只读环境变量交接
docs/               # 文档
scripts/            # build / install / self-check
```

## 许可

MIT or Apache 2.0

