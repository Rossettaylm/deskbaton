# DeskBaton 安装器与分发设计

日期：2026-10-18　状态：已确认

主设计见 `2026-10-16-deskbaton-design.md`，开机自启见 `2026-10-17-panel-settings-design.md`。冲突时以本文为准。

修订（2026-10-18）：tag `vX.Y.Z` 发 GitHub Release 供程序内更新；`main` 推送仍只上传 7 天 artifact。安装包文件名随 workspace 版本。见 `2026-10-18-in-app-update-design.md`，冲突时以该文为准。

修订（2026-10-19）：Windows 当前用户安装的实际落点是 `%LOCALAPPDATA%\DeskBaton\`。Tauri 2.12 NSIS 在 `installMode = currentUser` 时把 `$INSTDIR` 设为 `$LOCALAPPDATA\${PRODUCTNAME}`，不经过 `Programs`。安装器不改目录。程序内更新不靠这个路径决定是否可检查。

用户拿到的是安装包，不是裸 `deskbaton-gui` / `.exe`。装完后面板（进程内跑 agent）落在系统常规位置，并预置登录自启；Mac 另预置特权助手。主程序仍在用户登录会话里跑，不做 Windows Service、系统级 LaunchDaemon 里的 agent、或 systemd 系统服务。

## 1. 目标

Mac 和 Windows 用各自的安装包装上 DeskBaton。CI 构建并上传这两个安装包。Linux 安装器和 Linux 面板等 Mac + Windows 定型后再做。

## 2. 范围

**做**

- macOS：`.dmg` 里的 `DeskBaton.app`；`bundle.macOS.signingIdentity` 为 `-`，对整个 `.app` 做 ad-hoc 签名
- Windows：NSIS 当前用户安装器
- 安装或首次启动预置登录自启
- Mac 安装版用 `SMAppService` 登记特权助手，替换手跑 `deskbaton-cli sleep-helper install`
- CI：`check` 仍三平台跑测试；新增 Mac / Windows 的 `pack`，上传安装包
- 删掉 `windows-build.yml` 的裸 exe 产物

**不做**

- Linux 安装器、Linux 面板、Linux 开机自启
- 把 agent 做成 Windows Service 或 macOS 系统级 LaunchDaemon
- 改交棒、配对、剪贴板流程
- 把 `deskbaton`、`deskbaton-cli` 放进用户安装包
- Intel Mac、Windows arm64
- 公证、Authenticode（GitHub Release 见程序内更新设计）
- 改数据目录位置

开发时 `cargo run` / `cargo build` 仍出裸二进制，不作为分发产物。

## 3. 安装包与落点

| 平台 | 产物文件名 | 程序落点 | 数据目录（不改） |
|---|---|---|---|
| macOS | `DeskBaton-0.0.1-macos-aarch64.dmg` | `/Applications/DeskBaton.app` | `~/Library/Application Support/DeskBaton/` |
| Windows | `DeskBaton-0.0.1-windows-x64-setup.exe` | `%LOCALAPPDATA%\DeskBaton\` | `%APPDATA%\DeskBaton\` |

版本号与 workspace `version` 相同，升版本时文件名跟着改（当前示例为 `0.0.1`）。tag 发 Release 时的文件名与更新通道见 `2026-10-18-in-app-update-design.md`。

`tauri.conf.json`：`productName` 改为 `DeskBaton`（现在是 `DeskBaton Panel`）；`identifier` 保持 `io.github.rossettaylm.deskbaton.panel`；`bundle.active` 打开；`targets` 只有 `dmg` 和 `nsis`；`bundle.macOS.signingIdentity` 为 `-`。Windows `nsis.installMode` 为 `currentUser`。安装后 Windows 可执行文件名为 `deskbaton-gui.exe`（Tauri 按 Cargo 二进制名命名，不随 `productName`），装在 `%LOCALAPPDATA%\DeskBaton\`（Tauri NSIS `currentUser` 默认目录）；两者均经 `scripts/smoke-windows.ps1` 在 Windows runner 上实测。

`.dmg` 只负责把 `.app` 放到应用程序文件夹。不另做 `.pkg`：特权助手必须从 `.app` 里登记 `SMAppService`，`.pkg` 也省不掉系统设置那一次确认。不另做 MSI：当前用户 NSIS 不需要管理员，机器级安装用不上。

## 4. 预置：登录自启与特权助手

### 4.1 谁在何时写入

| 项 | Windows | macOS 安装版（从 `.app` 启动） | 开发版（`cargo run` 等裸二进制） |
|---|---|---|---|
| 登录自启 | NSIS `POSTINSTALL` 写 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 值名 `DeskBaton`，数据为 `"$INSTDIR\deskbaton-gui.exe"` | 首次启动 `SMAppService` 登记主程序为登录项 | 维持现有 LaunchAgent `com.deskbaton.gui` / 同一 Run 键 |
| 特权助手 | 无 | 首次启动 `SMAppService` 登记 LaunchDaemon，只执行现有两条完整 `pmset` 命令 | 维持 `deskbaton-cli sleep-helper` 写 sudoers |
| `autostart_seeded` | 首次启动时若 Run 键已在则置真；否则按面板设置设计 5.4 尝试开启 | 登录项登记成功后置真 | 不变 |

判定安装版：macOS 上可执行文件在 `.app` 包内。Windows 上 NSIS 已写好 Run 键，逻辑与「seeded 为假则尝试开启」兼容（写入已存在的同一值）。

登记成功后 `autostart_seeded` 为真，面板开关不再擅自重开。用户仍可在面板关掉自启。登录项已开时每次启动把目标刷新为当前可执行文件，避免开发版路径残留（面板设置设计 5.4 第 2 条）。

登录项 / Run 键写入失败：安装仍算成功。开关显示为关，底栏报中文原因。不阻断交棒。

### 4.2 Mac 特权助手（安装版）

助手是独立小二进制，放进 `DeskBaton.app`。LaunchDaemon plist 放在 `Contents/Library/LaunchDaemons/`，供 `SMAppService` 按文件名登记。助手只接受这两条完整命令，对应现有 sudoers 规则：

- `/usr/bin/pmset -a disablesleep 1`
- `/usr/bin/pmset -a disablesleep 0`

其它 argv 一律拒绝并退出非零。不接受任意按键、不读网络、不调 DDC。

`SMAppService` 登记会让用户到「系统设置 → 通用 → 登录项与扩展」允许一次。拒绝或登记失败：与现在 sudoers 缺失相同——`stay_awake` 只持防空闲断言并记警告，不阻断交棒。托盘 / 底栏提示用户去系统设置允许后台项目。

安装版的 `stay_awake` / `restore_sleep` 经助手执行 `pmset`，不再调用 `sudo -n`。开发版继续 `sudo -n` + sudoers。标记文件位置不变：`~/Library/Application Support/DeskBaton/stay-awake`。

`SMAppService` 与 sudoers 的 ObjC / 私有调用只放在 `baton-macos-sys/native`，Rust 侧只做 FFI。

退出进程时不注销登录项、不注销助手（否则每次退出等于关掉开机自启和合盖防睡）。注销只发生在用户于面板关掉开机自启（登录项）时；助手在应用还装在 `/Applications` 期间保持登记。

### 4.3 Windows 安装器钩子

`crates/baton-gui/windows/hooks.nsh`：

- `NSIS_HOOK_POSTINSTALL`：写上述 Run 键。
- `NSIS_HOOK_PREUNINSTALL`：删除该 Run 键。值不存在不算失败。

不在安装器里写 `state.toml`。不写 `HKLM`。不装 Windows 服务。

## 5. 升级与卸载

升级：覆盖同路径程序文件，不删数据目录（身份、`peers.toml`、`state.toml`、睡眠标记留下）。

Windows 卸载：删安装目录和 Run 键，保留 `%APPDATA%\DeskBaton\`。

macOS 没有卸载程序。用户把 `DeskBaton.app` 丢进废纸篓。丢进废纸篓不会跑脚本，助手和登录项可能残留：用户到「系统设置 → 通用 → 登录项与扩展」关闭 DeskBaton 的后台项目。若当时正持有 `SleepDisabled`，标记文件仍在数据目录；再装并启动一次会走现有 `restore_sleep`。不再安装时，文档写明可手动 `sudo pmset -a disablesleep 0`（仅当自己确认是 DeskBaton 留下的）。

## 6. CI

`ci.yml` 的 `check` 不变：`push` / `pull_request`，三平台 `fmt`、`clippy -D warnings`、`test`；Linux `--exclude baton-gui`。

新建 `.github/workflows/pack.yml`：

| | |
|---|---|
| 触发 | `main` 的 `push`；`workflow_dispatch`；tag `vX.Y.Z`（Release 见程序内更新设计） |
| 不触发 | pull request |
| 矩阵 | `macos-latest` → `--bundles dmg`（aarch64）；`windows-latest` → `--bundles nsis`（x64）。tag 发更新通道时 Mac 改为 `--bundles dmg,app`，见程序内更新设计第 6 节 |
| 构建 | `tauri-apps/tauri-action`，`projectPath` 为 `crates/baton-gui` |
| 上传 | `main` / 手动：第 3 节两个文件名，保留 7 天；tag：另发 GitHub Release |

`check` 未通过不阻止手动 `workflow_dispatch` 出包。PR 不打包。

删掉 `.github/workflows/windows-build.yml`。`main` 推送不建 GitHub Release。

Tauri / NSIS 打出的原始文件名若与第 3 节不同，`pack` 在上传前改名。

## 7. 未公证（ad-hoc 签名）

首版 CI 不配 Apple Developer 证书、不公证、不签 Authenticode。

macOS 必须对整个 `.app` 做 ad-hoc 签名：`bundle.macOS.signingIdentity` 为 `-`。不设此项时，链接器只给 Mach-O 打 `adhoc,linker-signed`，`Info.plist` 与 Resources 不进签名；Apple Silicon 上经浏览器下载后，Gatekeeper 报「已损坏」而不是「未识别的开发者」。ad-hoc 签整包后，用户仍要右键 → 打开，或到「系统设置 → 隐私与安全性」允许。安装包和程序都不剥隔离属性、不做其它绕过。

- **Windows：** SmartScreen 提示「已保护你的电脑」。用户点「更多信息」→「仍要运行」。

有 Apple Developer 与 Authenticode 证书后再加 Developer ID / 公证，不在本文实现。

## 8. 代码组织

```text
crates/baton-gui/tauri.conf.json          productName、bundle.active、targets、signingIdentity、nsis.installMode
crates/baton-gui/windows/hooks.nsh        Run 键写入 / 删除
crates/baton-gui/src/startup.rs           macOS 安装版走 SMAppService 登录项，开发版维持 LaunchAgent
crates/baton-power                        安装版经助手调 pmset，开发版维持 sudo -n
crates/baton-sleep-helper                 新 crate：仅允许第 4.2 节两条 pmset，exec /usr/bin/pmset
crates/baton-gui/macos/LaunchDaemons/     SMAppService 用的 plist
crates/baton-macos-sys/native             SMAppService 登记 / 查询 / 注销
.github/workflows/pack.yml                打安装包
.github/workflows/windows-build.yml       删除
```

不把安装、登录项、特权助手做成 `baton-core` 端口。不改 `PROTOCOL_VERSION`。不为安装器改交棒流程。

`baton-sleep-helper` 作为 Tauri `externalBin` 打进 `.app`。crate 在非 macOS 上是空库（不链 pmset），这样 Linux `cargo test --workspace --exclude baton-gui` 不必为助手装系统依赖。

## 9. 测试与验收

**单测**

- 助手：只放行第 4.2 节两条命令；其它 argv（缺参、多参、改开关、加别的子命令）拒绝。
- 开机自启策略：安装版首次启动在登录项已存在时把 `autostart_seeded` 置真且不重复「失败重试」以外的副作用；`autostart_seeded` 为真且系统已关时不得再开（沿用面板设置设计第 7 节）。
- 配置：`baton-gui` 单测读 `tauri.conf.json`，断言 `productName` 为 `DeskBaton`、`bundle.active` 为真、`targets` 含 `dmg` 与 `nsis`、`nsis.installMode` 为 `currentUser`、`bundle.macOS.signingIdentity` 为 `-`。CI `check` 在 macOS / Windows 上跑（Linux 排除 `baton-gui`）。

**CI**

- `check`：现有命令不变。
- `pack`：`main` 或手动触发后，两个 artifact 文件名与第 3 节一致。

**手动验收**

1. Mac：从 `.dmg` 把 app 放到 `/Applications`，过 Gatekeeper 后首次启动，系统设置里出现 DeskBaton 登录项；允许助手后，合盖切走再切回不睡（与现有 sleep-helper 验收相同）。
2. Mac：在系统设置拒绝助手，交棒仍能做，底栏或日志有警告。
3. Windows：装完注销再登录，托盘在，不必手开；面板「开机自启」为开。
4. Windows：卸载后 Run 键不在，`%APPDATA%\DeskBaton\` 还在。
5. 开发：`cargo run -p baton-gui` 仍能启动；已开自启时路径刷新为当前裸二进制，不指向已卸载的安装目录。

硬件相关项标 🔧，由用户确认后才在 `TODO.md` 打勾。

## 10. 风险

| 风险 | 应对 |
|---|---|
| 未公证，Gatekeeper 与 SmartScreen 仍拦截 | 第 7 节：ad-hoc 避免「已损坏」；用户右键打开；证书另做 |
| 用户拒绝 Mac 后台项目，合盖切回仍会睡 | 与现在 sudoers 缺失相同；警告，不挡交棒 |
| `.app` 丢进废纸篓后助手或 `SleepDisabled` 残留 | 第 5 节：系统设置关掉后台项目；`restore_sleep` / 手动 `pmset` |
| 开发版 LaunchAgent 与安装版 SMAppService 并存 | 按是否在 `.app` 内分流；已开则刷新为当前路径 |
| `pack` 在 PR 上太慢 | PR 不跑 `pack` |
| Linux 以后要安装器 | 本文不做；Mac + Windows 定型后另立设计 |

## 11. 对既有设计的修订

- 主设计 5.3：安装版特权来源改为本文第 4.2 节的 `SMAppService` 助手；开发版仍用 sudoers。
- 主设计第 9 节 CI：补 `pack` 出安装包；不再分发裸 exe。
- 主设计第 10 节 M2：退出条件加上「Mac / Windows 能用安装包装上，登录后面板自起」。
- 面板设置 5.4：macOS 安装版登录项改为 `SMAppService`；Windows 安装器可先于首次启动写 Run 键。`autostart_seeded` 语义不变。
