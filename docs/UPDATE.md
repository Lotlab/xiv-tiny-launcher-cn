# 版本检查与下载 / 更新

启动器现在除了登录，还能检查并更新游戏本体。实现在新 crate `crates/patcher/`，
`launcher` 只负责 CLI 与终端进度。

## 分层

```
crates/patcher/
├── build.rs              打包时注入密钥（缺则编译失败）
├── src/keys.rs           env!/include_str! 读回（源码与仓库里没有明文）
├── src/crypto/           本地版本元数据用的 LSB-first DES 变体 + 3DES-EDE
├── src/version_meta.rs   game/LocalVersion3.xml 编解码（magic + 3DES + 长度）
├── src/version.rs        版本比较、UpdateDecision、本地版本读取
├── src/hash.rs           MD5 / hex 小工具（全 crate 共用）
├── src/relpath.rs        清单/元数据路径规范化与越界校验（zip-slip 防护）
├── src/cdn/              CDN 网络层
│   ├── ver2.rs             ver2.dat（版本 + packages 链）
│   ├── auth.rs             v3ctrl.xml + RSA 公钥解密 + 鉴权 URL
│   ├── filelist.rs         client_all_files_list.dat + 单文件 URL
│   └── mod.rs              Cdn 客户端 + 下载（Range 续传 / size+MD5 校验）
├── src/download.rs       全量校验计划 + 下载编排（重试 + 鉴权刷新 + Progress）
├── src/patch.rs          补丁链 / 补丁清单 / zip 解压 / delta 元数据
├── src/delta.rs          rxdelta 原地打补丁（delta/origin/result 三道 MD5 校验 + 幂等）
└── src/update.rs         增量编排（多跳 → 链尾补齐 → 写版本）
```

网络与登录协议（`sdo-client`）分开：CDN 需要长超时、流式落盘、断点续传，证书/代理策略也不同。
`game_id` / `build_id` 由调用方传入，启动器用 `sdo_client::GAME_APP_ID` /
`sdo_client::BUILD_ID`（与区服表路径同一个 build id）；`launcher` 里有测试断言
`patcher::cdn::BUILD_ID` 与它一致（后者只给示例当默认值）。

## 密钥注入

两把密钥都不进版本库，`build.rs` 在打包时读取，缺任一即**编译失败**：

| 密钥 | 环境变量 | 文件 | 说明 |
|---|---|---|---|
| CDN RSA 公钥 | `SDO_FFXIV_CDN_RSA_PUBLIC_KEY` | `keys/cdn_rsa_public.pem` | `v3ctrl.xml` 鉴权用（PEM） |
| 本地元数据 3DES 密钥 | `SDO_FFXIV_META_DES_KEY` | `keys/meta_des.key` | `LocalVersion3.xml` 用（32 位 hex） |

也支持 `*_FILE` 变体指定路径。`keys/` 已 gitignore；`scripts/build.sh` 会在打包前解析并校验。

## 算法要点（逆向自官方 `Launcher.dat` / `ClientAllFilesCheck.dll`）

- **版本检查**：`ver2.dat` 的 `areas[0].max` 是最新 internal 版本；`packages` 是一条
  `from → to` 线性链；`versionView` 下划线前是 display 版本。
- **鉴权**：`v3ctrl.xml`（GBK）里的 `<md5key>` / `<referer>` 是 RSA 公钥加密后的 hex，
  用公钥运算 + PKCS#1 v1.5 type-1 去填充解出；`cdn_token = client[..16] + server-let[-16..]`；
  按 `iplist@type` 把 `MD5(token+path+time_hex) / time_hex` 前缀插进 URL。
- **文件清单**：首行 `hash_base_path|identifier|hash_id`，行 `path|size|md5`；
  单文件 URL 的末段是 `MD5_UTF16LE("{identifier}_{hash_id}_{path}")`。
- **差分包**：补丁 zip 里有 `patch_delta_direct.dat`（XML 元数据）与 `Pkg\game\*.delta`；
  元数据给出 `origin_md5`（打前）/ `result_md5`（打后）。
- **本地版本元数据**：`game/LocalVersion3.xml` 磁盘格式是
  `FF FF FF FF + 3DES-ECB 密文 + 小端明文长度`；明文是 XML 包一段 JSON
  （`product_name` / `version.v` / `version.view`）。
  **注意**：这个 DES 不是标准 DES——表是标准 DES 表，但位序是每字节 LSB-first、
  S 盒输出低位在前，必须照抄（`crypto/des.rs`）。

## CLI

```sh
sdo-ffxiv-launcher                          # 默认：自动检查 → 有更新就增量 → 登录
sdo-ffxiv-launcher --check-update           # 只检查后退出（与 --force-full / --no-update 互斥）
sdo-ffxiv-launcher --no-update              # 跳过更新直接登录（离线/调试）
sdo-ffxiv-launcher --yes                    # 跳过「是否现在更新」的询问（脚本/无人值守）
sdo-ffxiv-launcher --force-full             # 强制全量：先校验、再问是否下载（--verify 是别名）
sdo-ffxiv-launcher --insecure-cdn           # CDN 跳过 TLS 校验
```

## 更新阶段的行为

- **增量不做全量校验**：链尾只补「补丁声明要改但没打成功」和「目标清单里有、本地
  没有」的文件，外加 20 字节的 `game/ffxivgame.ver`（`read_local` 的回退值）。
  全量校验（实测要读 ~118 GB）只出现在 `--force-full` / `--verify` 与「本地没有
  internal 版本」的自动全量里，而且是**先校验、报出缺口，再问是否下载**。
- **更新不阻断登录**：版本检查失败（CDN 抖动 / 403 / 证书）、增量链走不通
  （本地过旧 / CDN 无对应包，在下载前预演）、非交互式下无法确认更新，都只提示后
  继续登录；确认下载之后才发现 `ffxiv_dx11.exe` 在运行也只跳过本次更新（文件被占用），
  不拦登录。只有真正开始下载 / 打补丁之后的错误才中止。`--check-update` 例外：
  用户就是来查版本的，失败如实报错退出。
- 打 delta 前先校验 delta 自身（元数据里的 `delta_md5`），再校验 `origin_md5` /
  `result_md5`；任一不符都只记日志并交给链尾整文件补下。
- 单个 zip 条目解不出来（不支持的压缩方法 / 条目损坏）不终止整次更新：该文件的
  delta 缺席，链尾按目标清单补下。

## 路径安全

清单（`client_all_files_list.dat`）、补丁清单 JSON、补丁 zip 条目名与
`patch_delta_direct.dat` 里的路径都来自网络，会直接参与 `Path::join` 落盘，
所以统一过 `relpath::safe_rel_path` / `is_safe_local_rel`：拒绝 `..`、盘符与
NTFS ADS（含 `:`），落盘路径额外拒绝前导 `/`（`Path::join` 遇到绝对路径会整段替换）。
清单里的路径**保持原始反斜杠形式**存着——单文件 URL 的 MD5 按原始串算。

## 更新确认

**全量是「先校验、再询问、最后下载」**（`--force-full` / `--verify` 与自动更新里
「本地没有 internal 版本」的全量分支都一样）：先把整份清单跟本地逐个比对 size + MD5
（`patcher::verify_full_with`，实测要读 ~118 GB），打印「清单 N 个文件，本地通过 M 个，
需下载 K 个（约 X GB）」，然后才问一次「是否现在更新？[Y/n]」。用户是先知道缺口多大
再决定要不要下；全部通过就不问，直接写版本元数据收工（连 CDN 鉴权都不取）。
增量更新不做全量校验，没有可先算的缺口，所以还是下载前问一次。

- 回车 / `y` / `yes` / `是` → 开始下载；`n` / `no` / `否` → **跳过本次更新**，
  照常登录（等价于 `--no-update`，游戏版本可能与服务器不一致）。
- 不设超时：没有输入就一直等，避免误触发几十 GB 的下载。
- `--yes` 直接放行，不再询问（脚本用）；**校验照做**，缺多少照样打印。
- stdin 不是终端（管道 / 无人值守 / 双击）且没给 `--yes` 时，闸门在**开始校验之前**
  就给结论，不会白读一遍整份安装：**自动更新**跳过本次更新并继续登录（提示加
  `--yes`）；`--force-full` 因为用户显式要求全量，会中止并报错要求 `--yes`。

更新前会**预演增量链**（本地 internal → CDN 目标），链走不通就直接跳过更新继续登录，
不会先让用户确认再失败（全量的「校验」也是同一种思路：先算清楚再问）。更新失败会中止
（版本检查失败 / 链走不通 / 尚未开始下载的失败不中止，见上）；`--check-update` /
`--self-check` 不做实际下载。更新前会检查 `ffxiv_dx11.exe` 是否在跑（确认下载之后才查）；
打 delta / 下 zip / 链尾补齐前会预检磁盘剩余空间。

## 手工验证

```sh
# 版本检查（可指定 --game-dir）
cargo run -p patcher --example check_update -- --game-dir /path/to/ffxiv

# 鉴权 + 单文件下载
cargo run -p patcher --example download_one

# 补丁链 + 补丁清单（--extract 会下 zip 并解 delta）
cargo run -p patcher --example incremental_info -- --from 0.0.0.27 --extract
```

`cargo test -p patcher` 含 12 组 DES 二进制向量、元数据往返、路径越界校验、真实
`xdelta3` 生成的 delta 端到端（fixture 提交在 `tests/fixtures/`，不依赖系统里有没有 `xdelta3`）。

## 已知点

- **边缘 403 的处理**（实测结论）：GSLB 在多家边缘节点间轮换，偶发 403 与鉴权公式无关。
  重试策略：只有 458（配置失效）才刷新鉴权重试；403/5xx 不刷新，
  而是先用 HTTPDNS（`cdn/httpdns.rs`，公共 DoH 取全量边缘 IP，等价官方
  `CURLOPT_RESOLVE` 的 pin 住轮换；DoH 不可用回退普通重连）换节点，
  再按 10s/30s/60s 退避，并在主备 host（`ver2.backupBaseUrl`）之间轮换。
  403 响应体长度会记进日志当指纹：17≈公式错 / 11≈时间戳旧（本机慢 >30 分钟）/ 0≈边缘拦截。
- CDN 走**系统代理**：跟随 `http_proxy` / `https_proxy` 等环境变量（reqwest 默认），
  `no_proxy` 里的域名照常绕过，没有单独的代理开关
- 全量是 ~118 GB；默认走增量（实测一跳只改 32 个文件 / 37 MB），全量只在全新安装/修复时用。
- 增量链尾会按目标清单查「文件是否存在」，并校验 20 字节的 `game/ffxivgame.ver`；
  大文件的内容校验（MD5）只在 `--force-full` 时做。`game/LocalVersion3.xml` 因落盘是密文
  而跳过清单校验，由更新流程最后**原子写入**。
- `game/ffxivgame.ver` 与其它游戏文件一样在清单里；增量时链尾会按清单校验/补下它，
  启动器不单独拼写它的内容。
- **磁盘预检**：打 delta 前按 rxdelta 的窗口布局算实际需要的额外空间（原地重写要落一份
  journal，约等于变更量，加上文件增长量；回退 temp+rename 才需整个目标大小）；下 zip 前按未缓存部分预检。
- 断点续传不需要状态文件：patch zip 按 MD5 缓存在 `_update/zips/`、普通文件按 size+MD5
  跳过（残缺用 `Range` 续传）、delta 靠 rxdelta 的幂等跳过 + journal 恢复；
  中断的全量安装等价于 `--verify`（校验并修复）。`_update/` 在**游戏成功启动后**清理。
- **zip 压缩方法**：只启用了 `Stored` + `Deflate`（`zip` crate 的 `deflate` feature）。
  实测线上补丁包（37 MB / 32 条 delta）就是这两种，能正常解出。若哪天 CDN 改用
  Deflate64 / zstd / bzip2 / lzma，单个 delta 会解不出来——此时不再中止更新，
  而是退回「整文件下载」（见上）。
