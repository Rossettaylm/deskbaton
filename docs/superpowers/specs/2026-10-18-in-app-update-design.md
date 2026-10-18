# DeskBaton 程序内自动更新设计

日期：2026-10-18　状态：已确认

主设计见 `2026-10-16-deskbaton-design.md`。安装器见 `2026-10-18-installer-design.md`。本机卡片见 `2026-10-17-panel-settings-design.md`。冲突时以本文为准。

## 1. 目标

安装版面板能自己发现 GitHub 上的新版本；用户确认后下载、验签、覆盖安装并重启。日常 `main` 推送仍只打 7 天 artifact，不进更新通道。

## 2. 范围

**做**

- 仅安装版面板（Mac `.app`、Windows NSIS 装上的 `DeskBaton.exe`）
- `vX.Y.Z` tag 发 GitHub Release：新人用的 `.dmg` / NSIS，加上更新器用的签名包和 `latest.json`
- 启动后后台检查一次；本机卡片可再查
- 有新版本则提示，点按钮后下载、minisign 验签、覆盖同路径、重启
- 公钥进仓库，私钥进 GitHub Secrets

**不做**

- Linux、开发版 `cargo run`、无界面 `deskbaton`
- 静默升级、强制升级、跨大版本迁移向导
- Apple 公证 / Authenticode（仍按安装器设计，有证书另做）
- 每次 `main` 推送发 Release
- 把「跳过此版本」写入 `state.toml`
- 改交棒协议、把互联端口暴露到公网
- Android / iOS（Tauri 2 能做移动端，更新器插件不能；手机端是另一个产品面，P3 另立）

**版本：** tag `v0.0.2` 必须等于 workspace `version = "0.0.2"`，对不上 CI 失败。安装包文件名随版本变，例如 `DeskBaton-0.0.2-macos-aarch64.dmg`。

**网络：** 机器互联仍只走局域网 TLS。检查更新是另一次对 GitHub 的 HTTPS GET。失败只记日志或底栏，不挡交棒。不升 `PROTOCOL_VERSION`。

## 3. 安装版判定

与安装器设计同一条线，写在 `startup.rs`，更新逻辑复用：

| 平台 | 安装版 | 开发版 |
|---|---|---|
| macOS | 可执行文件的祖先路径带 `.app`（现有 `exe_is_bundled`） | `cargo run` 的 `target/.../deskbaton-gui` |
| Windows | 可执行文件在 `%LOCALAPPDATA%\Programs\DeskBaton\` 下 | `target\...\deskbaton-gui.exe` |

开发版不检查网络、本机卡片不画「版本」行。

## 4. 检查与安装

进程内一份 `UpdateSlot`（`Mutex`），同一时刻只跑一轮检查或安装。

```text
启动（安装版）
    → 窗口起来后 spawn 检查 latest.json
        ├ 已是最新 / 网络失败 → 只记日志，槽位 kind=none
        └ 有新版本 → 槽位 kind=available
              emit update-available
              底栏 notice「发现新版本 vx.y.z」
              通知开关为开则再弹一次系统通知（同一句；点通知只打开面板）
本机卡片「检查更新」
    → 再查一次；已最新则底栏「已是最新版本」；失败则底栏中文原因
点「升级到 vx.y.z」
    → 底栏「正在下载更新…（升级会短暂退出）」
    → 验签、覆盖、AppHandle::restart()
```

比较：semver，只升不降。清单地址：

`https://github.com/Rossettaylm/deskbaton/releases/latest/download/latest.json`

自动检查的失败不写底栏。同一版本的「发现新版本」底栏和系统通知每进程只发一次；被其它底栏文案盖掉后不再自动写回。按钮在这次进程里一直保持「升级到 vx.y.z」，直到重启或升完。不另做关闭按钮，不写 `state.toml`。

覆盖安装不删数据目录（身份、`peers.toml`、`state.toml`、睡眠标记留下），与安装器设计第 5 节一致。下载中热键和交棒照常。升级会退出当前进程：合盖 Mac 上会松开 `AwakeGuard`，可能睡着。不为此先交棒，也不拦升级。

| 情况 | 自动检查 | 手动检查 / 升级中 |
|---|---|---|
| 已是最新 | 只记日志 | 底栏「已是最新版本」 |
| 网络失败、清单无效 | 只记日志 | 底栏说明原因 |
| 签名不对 | — | 不安装，底栏「更新包校验失败」 |
| 写入失败 / 重启失败 | — | 底栏说明；已写下的文件不回滚 |
| 正在升级时再点 | — | 底栏「正在升级…」，不另开一轮 |

进度：`emit` `update-progress` `{ loaded, total }`。有 `total` 时底栏「正在下载更新 12 / 40 MB」；没有则保持「正在下载更新…（升级会短暂退出）」。

## 5. 面板

不新开窗口、不加 tab。本机卡片在「剪贴板」下面加一行，颜色仍用 `--ink` / `--mute`，不用橙色。右侧是动作按钮 `.btn`（空闲为默认，有更新为 `primary`），与设备页「同意 / 连接」同一套；不要热键那种凹槽。

| 状态 | 这一行 |
|---|---|
| 空闲 / 已最新 | 左侧「版本」，右侧按钮「检查更新」 |
| 发现新版本 | 左侧「版本」，右侧按钮「升级到 vx.y.z」 |
| 正在下载 / 安装 | 按钮不可点，文案「正在升级…」 |
| 开发版 | 不画这一行 |

底栏右侧继续显示当前版本号。升级确认就是点这个按钮，不弹系统对话框。

前端继续 `invoke`，不加 npm。命令：`update_status`（只读槽位，不发网络；返回是否安装版以及 `idle` / `available` / `installing`）/ `check_update` / `install_update`。事件：`update-available`、`update-progress`。槽位不是 available 时调用 `install_update`，底栏「没有可用更新」。

## 6. CI、密钥、Release

`pack.yml` 仍打 Mac `.dmg` + Windows NSIS：

| 触发 | 做什么 |
|---|---|
| `main` 推送、`workflow_dispatch` | 上传 7 天 artifact，不发 GitHub Release |
| 推送 tag `vX.Y.Z`（`v` + 三段数字） | tag 去掉 `v` 后必须等于 workspace `version`；签名并上传更新器产物；同一 Release 挂 `.dmg`、NSIS、`latest.json`，直接发布（不当草稿） |

安装包文件名带 workspace 版本，不再写死 `0.0.1`。Windows 更新清单优先 NSIS。Linux 仍不打包。

macOS 更新器要 `.app.tar.gz`，不是 `.dmg`。Tauri 2 把 `app` 算更新器目标，`dmg` 不算：tag 时 Mac 必须 `--bundles dmg,app`，否则会警告 `no updater-enabled targets`、不写 `.sig`，`latest.json` 没有 `darwin-aarch64` / `darwin-aarch64-app`，安装版检查更新报 fallback platform。Windows 的 `nsis` 本身就是更新器目标，保持 `--bundles nsis`。tag job 若找不到本平台 `.sig` 必须失败，不能只打出安装包。

仓库里 `bundle.createUpdaterArtifacts` 为 `false`，日常 pack 和本地打安装包不需要私钥。仅 tag job 用 `--config` 打开它，并注入：

- `TAURI_SIGNING_PRIVATE_KEY`
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`（可空）

`tauri.conf.json` 写 `plugins.updater.pubkey` 和上述 endpoint。私钥不进仓库。没配密钥时 tag job 必须失败，避免 `latest` 上出现不能验签的包。tag job 需要 `contents: write`。

实现时用 `tauri signer generate` 生成密钥对：公钥写入配置，私钥由用户放进仓库 Secrets。Agent 不生成、不提交私钥。

**发可升级版本：** 先改 workspace `version` 并提交，再打 annotated tag `vX.Y.Z` 并 `git push origin vX.Y.Z`。`scripts/ship.sh` 不自动打 tag。

## 7. 代码组织

```text
crates/baton-gui/src/update.rs      安装版判定入口、检查、下载安装、重启
crates/baton-gui/src/startup.rs     Windows 安装版路径判定
crates/baton-gui/src/commands.rs    update_status / check_update / install_update
crates/baton-gui/ui                 本机卡片「版本」行
crates/baton-gui/tauri.conf.json    plugins.updater；createUpdaterArtifacts 默认 false
.github/workflows/pack.yml          tag v* 发 Release；文件名用 workspace 版本
Cargo.toml                          tauri-plugin-updater（MIT / Apache-2.0，兼容 GPL-3.0）
```

不把更新做成 `baton-core` 端口。用 Rust 侧 `UpdaterExt` + `AppHandle::restart()`，不引进 `tauri-plugin-process`。错误文案中文。

## 8. 测试与验收

**单测**

- 安装版路径：Mac `.app` 为真，`target/debug/deskbaton-gui` 为假；Windows `%LOCALAPPDATA%\Programs\DeskBaton\DeskBaton.exe` 为真，`target\release\deskbaton-gui.exe` 为假
- 开发版检查直接返回不支持，不发网络
- `tauri.conf.json`：有 endpoint 与公钥字段、`createUpdaterArtifacts` 为 false
- `pack.yml`：有 `v*` tag 触发、版本一致性校验、文件名来自 workspace 版本而不是字面 `0.0.1`；PR 仍不打包；tag 的 Mac 参数含 `dmg,app`，缺 `.sig` 则失败

**CI**

- `check` 命令不变；Linux 继续排除 `baton-gui`
- tag 发 Release 的完整链路在 GitHub 上跑，不在 `cargo test` 里打安装包

**手动验收**

1. 低版本安装版启动后底栏提示新版本，本机卡片可升级；确认后重启，版本号变新，数据目录还在。
2. `cargo run -p baton-gui` 没有「版本」这一行。
3. 已是最新时手动检查：底栏「已是最新版本」。
4. 断网：自动检查无提示；手动检查底栏报错，交棒仍可用。
5. 🔧 Mac：升完后若 Gatekeeper 拦截，记进验收，不在本轮做公证。

## 9. 风险

| 风险 | 应对 |
|---|---|
| 未公证，升完被 Gatekeeper / SmartScreen 拦 | 与现网安装相同；验收记下，证书另做 |
| tag 与 `version` 不一致 | CI 直接失败 |
| Secrets 没配就把包发到 latest | tag job 没私钥则失败 |
| 合盖 Mac 上升级松开 `AwakeGuard` | 文案写明会短暂退出 |
| 矩阵两台机器抢着写 `latest.json` | 沿用 tauri-action 合并清单 |

## 10. 对既有设计的修订

- 安装器第 2 节「不做 GitHub Release」删除；改为 tag `vX.Y.Z` 发 Release，见本文第 6 节。`main` 推送仍只上传 7 天 artifact。
- 安装器第 3 节文件名随 workspace 版本，不再写死 `0.0.1`。
- 安装器第 6 节：`pack.yml` 增加 tag 触发；「不建 GitHub Release」删除。
- 面板设置第 4 节：本机卡片加「版本」行，见本文第 5 节。
- 主设计：安装版面板可经 GitHub Release 更新；检查更新的 HTTPS 不等于把交棒端口暴露到公网。
