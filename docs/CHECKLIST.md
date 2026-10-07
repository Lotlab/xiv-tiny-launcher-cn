# 联调清单

> 自动化部分：`scripts/self-check.sh`（等价 `launcher.exe --self-check`），输出留档
> `docs/SELFCHECK-output.txt`。手工部分必须在真实登录 + 真实启动后核对，**⑤ 与 ⑦ 是关键项**。
>
> `--self-check` 的判定分四档，**只有 `PASS` 表示本次运行已自动验证**：
>
> | 判定 | 含义 |
> |---|---|
> | `PASS` / `FAIL` | 本次运行当场验证通过 / 不通过 |
> | `SKIP(需实测)` | 需要真实登录或游戏大厅，本工具测不了 |
> | `MANUAL(需人工)` | 只能人工核对（抓包、任务管理器、进程内），**不是通过** |

## 一次完整联调的推荐顺序

1. `scripts/build.sh` 产出 `dist/launcher.exe`、`dist/sdologinentry64.dll`。
2. 安装（**注意落点**，见 [DELIVERY](DELIVERY.md)）：`scripts/install.sh "C:\Games\FFXIV"`，
   它会备份非自研的 `<根>\sdo\sdologin\sdologinentry64.dll` 后覆盖，并把启动器放到 `<根>\`。
   `sdologinsdk64.dll`、`SdoLoginComServer.exe` 不需要安装。放错目录会得到 `5003 账号认证错误`，
   且 `sdologinentry.log` 不会生成。
3. 把 `launcher.exe` 放到 `<安装根>/`（与 `game/` 同级），运行：
   - 首次：`launcher.exe --mode qr`（无 `device.json`，走 QR）
   - 之后：`launcher.exe`（`auto`：有 `keepLoginKey` 先 `fastInLogin`）
4. 扫码后观察启动器输出 `SSO 换票成功`、`游戏已启动（pid=…）`。
5. 按下面清单逐项核对。

## 逐项

> **QR 链**：扫码成功并落盘 `keepLoginKey`；**fast 链**：`auto` 模式走 `fastInLogin` 免扫码成功；
> 两条链都完成 SSO 换票并启动 `<安装根>\game\ffxiv_dx11.exe`。
> 对应日志行：`fastInLogin 成功` / `SSO 换票成功 ticket1=…sndaId1=…` /
> `BSTR1==ticket1 / BSTR2==sndaId1 / AREAID=7` / `CreateProcess 成功 pid=…`。
> DLL 落在正确落点 `<根>\sdo\sdologin\` 时即可正常登录大厅，人工项 ⑤⑥⑦ 即全部核对通过。
> 另注：附属 `getMessageFile` 请求返回 HTTP 400（登录前后各一次）——
> 该请求是 fire-and-forget 且本实现不发 `exp_ff14` Cookie，不阻断。

| # | 项 | 自动化 | 手工核对方法 |
|---|---|---|---|
| 1 | 设备一致性：同会话 `deviceId/macId/epName/epIp` 一致 + `MD5(macId)==SEG0` | ✅ self-check ① | 抓包对比每个请求的四个参数是否同值 |
| 2 | Cookie：抽查 20+ CAS 请求无 Cookie 发出 | ✅ self-check ②（自有头集合）+ 抓包人工 | Wireshark 过滤 `http.request` 看 CAS 请求头；确认无 `Cookie:`、`CASCID` 全丢 |
| 3 | QR：扫码得 T0 非空 + 保留 `guid0` | ⚠️ 仅 getGuid/getCodeKey | 日志出现 `codeKeyLogin 成功`、`getGuid 成功 guid=…` |
| 4 | SSO：`ticket1 != ticket0` 即换票（相同记 WARN 不阻断） | ❌ | 日志 `SSO 换票成功 ticket1=…`；若 WARN 则复查 `areaId/authorization` |
| 5 | **票据（关键）**：`BSTR1 == ticket1`，BSTR2 = `sndaId1`（系统 Cookie 已移除，不再核对 `CASTGC`） | ❌ | 启动器 `assert_delivery`（env 与内存一致）+ `sdologinentry.log` 的 `GetTicket 交付` 行 |
| 6 | 命令行：`SDO_FFXIV_AREAID == SSO areaId == base AreaID`；子进程 env 四变量齐备 | ✅ self-check ⑥ | Process Explorer → 游戏进程 → Environment 页，4 个 `SDO_FFXIV_*` |
| 7 | **启动（关键）**：明文 `base` + `ffxiv_dx11.exe` + 工作目录 `game/`，能进大厅 | ❌ | 任务管理器命令行逐字段对照；进大厅即通过 |
| 8 | 回退：删 key 走 QR；fast 非零不重发；push send 非零从 cancel 起重试最多 3 次转 QR | ⚠️ 模板断言 | 删除 `device.json` 的 `keepLoginKey` 后运行；观察日志分支 |
| 9 | 登出：`Logout[7]` 清本进程 env，`device.json` 的 key 保留 | ✅ DLL 单测（self-check 标 `MANUAL`） | 游戏内登出后看 `sdologinentry.log`；`device.json` 内容不变 |
| 10 | 未知槽位：记日志不崩 | ✅ DLL 单测 | `sdologinentry.log` 里未知 IID/槽位记录按需补齐 |
| 11 | 只验明文 `base`（加密串形态不在首版范围） | `MANUAL`（self-check 只打印待核对内容） | 任务管理器可见 `Dev.LobbyHost01…` 明文 |

## 游戏拒票时

交接给游戏的必须是换票结果 T1。若游戏拒票，先核对 SSO 换票口径（`areaId`/`authorization`/两段
`productVersion`），不要改用 T0 交接——T0 只用于换票与登录后附属请求。

## 日志位置

| 日志 | 路径 |
|---|---|
| 启动器 | `%TEMP%\SdoFfxiv\launcher.log` |
| 游戏侧 DLL | `%TEMP%\SdoFfxiv\sdologinentry.log` |

`--log-file <path>` 可覆盖启动器日志路径；`%TEMP%` 不可写时回退到 EXE 自身所在目录。

两份日志均已脱敏：`ticket/tgt/keepLoginKey/guid/codeKey/authorization` 只留前 6 位与长度。
