# DeskBaton 面板本机设置设计

日期：2026-10-17　状态：待实现

主设计见 `2026-10-16-deskbaton-design.md`，显示器面板见 `2026-10-16-display-panel-design.md`，配对见 `2026-10-17-pairing-design.md`，零配置见 `2026-10-17-zero-config-design.md`。本文落实 M2 的「开机自启、通知」，并在面板上提供本机名称与切换热键。冲突时以本文为准。

配对设计第 2 节「不做」中的「系统通知、开机自启」改由本文实现。

## 1. 目标

面板「设置」页日常这几项不用再手改文件：开机自启、系统通知、本机名称、切换热键、剪贴板。改完立刻生效。

`config.toml` 这一轮开始不再作为写入目标，为以后下掉它做准备。现有手写字段只作只读回退。

## 2. 范围

**做**

- 面板「设置」页：名称、热键、开机自启、通知、剪贴板
- 名称、热键、通知写入 `state.toml`；开机自启只改系统登录项
- 系统通知接现有 `Notifier`：打开时弹系统通知，关掉时只打日志和面板状态行
- 开机自启默认开：面板首次成功种入登录项后不再擅自打开
- 无界面 `deskbaton` 读取 `state.toml` 的名称和热键，不种登录项、不弹系统通知

**不做**

- 回写或扩展 `config.toml`（含通知开关、开机自启字段）
- 保持唤醒、端口、输入口编码等其它配置项的面板编辑（剪贴板见 `2026-10-18-clipboard-realtime-design.md`）
- 改名称时主动断开已有连接
- 独立设置窗口；`⋯` 菜单不承担这四项
- Linux 开机自启（面板本就不在 Linux 分发）

## 3. 数据放哪

| 项 | 真相 | 读取顺序 |
|---|---|---|
| 开机自启 | 系统登录项 | 打开面板时问系统 |
| 名称、热键、通知、剪贴板 | `state.toml` | 名称/热键：`state.toml` > `config.toml`（只读）> 默认值；通知、剪贴板同步、剪贴板页：`state.toml`（缺省为开）；未发上限缺省 10 |
| 是否已种过登录项 | `state.toml` 的 `autostart_seeded` | 仅程序使用 |

与输入源编码相反：编码仍是配置优先于学到的值（零配置设计第 4.3 节）；名称和热键是 `state.toml` 优先，因为面板是今后的写入入口。

默认值：名称缺省为主机名；热键缺省为 `Ctrl+Alt+Space`（与现有 `Config` 默认相同）；通知缺省为开。

### 3.1 `state.toml`

在现有 `LocalState` 上增加字段，全部有默认值，继续 `deny_unknown_fields`：

```toml
input_code = 16                 # 已有：学到的本机输入源编码
name = "macbook"                # 可选；字段不存在 = 未在面板保存过
hotkey = "Ctrl+Alt+B"           # 可选；字段不存在 = 未在面板保存过
notify = true                   # 缺省 true
clipboard_sync = true           # 缺省 true；见剪贴板实时设计
clipboard_tab = true            # 缺省 true；只控制 tab 显隐
clipboard_outbox_limit = 10     # 缺省 10；每端发件箱上限，1–50
autostart_seeded = true         # 缺省 false
```

- `name` 缺省：回退 `config.toml` 的 `name`，再回退主机名。
- `name = ""`：用户在面板里清空了名称，用主机名，不再回退 `config.toml`。
- `hotkey` 缺省：回退 `config.toml` 的 `hotkey`，再回退 `Ctrl+Alt+Space`。
- 面板保存名称或热键时写入对应字段，即使值等于默认值也写，以便之后下掉 `config.toml` 仍能复现用户选择。

合并逻辑放在 `baton-core`（纯函数，可单测）。`Agent::start` 用合并后的名称和热键，这样无界面程序也能读到面板写过的值。

### 3.2 写入规则

对 `state.toml` 的所有写入都走同一条「读-改-写」：先加载完整 `LocalState`，改要改的字段，再保存。学习线程写入 `input_code`、面板「重新识别本机接口」、本机卡片改偏好，都走这条路径。禁止再构造只含 `input_code` 的 `LocalState` 覆盖整个文件。

先写盘再改运行中状态。写盘失败则运行中状态不变，底栏报中文原因。两条例外：开机自启是系统 API 成功后再落 `autostart_seeded`；热键是新组合注册成功后再写盘，写盘失败则卸掉新热键、重新挂上旧的。

### 3.3 不写 `config.toml`

即使机器上已有手写 `config.toml`，面板保存这三项也不回写它。用户若之后再手改 `config.toml` 的 `name` / `hotkey`，只有对应字段在 `state.toml` 里仍缺省时才会生效。

## 4. 本机卡片

面板宽仍 360px，高随这张卡增加。「本机」卡在「设备」和底栏之间，标题「本机」，副标题不写技术参数。

| 行 | 控件 | 行为 |
|---|---|---|
| 名称 | 文本框；空时 placeholder 为主机名 | 失焦或回车保存；内容相对打开时没变则不写盘 |
| 切换热键 | 显示当前组合；点一下进入录制 | 按下新组合即保存；Esc 取消录制，保持原值 |
| 开机自启 | 开关 | 反映系统登录项是否指向本程序 |
| 通知 | 开关 | 反映 `state.toml` 的 `notify` |
| 剪贴板 | 开关 | 反映 `state.toml` 的 `clipboard_sync`；见实时设计 |
| 剪贴板页 | 开关 | 反映 `clipboard_tab`；关掉只藏 tab |
| 未发上限 | 数字 | 反映 `clipboard_outbox_limit` |

录制热键期间，这次按键不得触发交棒。格式与现有 `hotkey::parse` 相同，例如 `Ctrl+Alt+Space`。

视觉沿用显示器面板第 7 节：深色卡片、开关与输入用现有 `--ink` / `--mute` / `--line`，不用橙色（橙色只留给当前信号）。

## 5. 立刻生效

### 5.1 名称

1. 写入 `state.toml`。
2. 本机面板标题、「电脑」卡片里的本机名立刻改。
3. 重新注册 mDNS，属性 `name` 为新名称。
4. 协调器持有的本机 `DeviceId`、链路后续握手的 `hello.device_id` 都改为新名称。已有连接不断开；对端按指纹认人，档案里的名称在下次握手时更新（配对设计第 5 节第 2 行已有「名称变了就更新档案」）。

协调器、发现、链路若没有「更新本机名称」的入口，补内部方法或内部事件；不为此升 `PROTOCOL_VERSION`，也不发新的线上消息。

### 5.2 热键

1. 在 Tauri 主线程解析并注册新热键（必须主线程，与现在相同）。新的注册成功之前不丢掉旧的，避免中间没有热键。
2. 注册成功后再写入 `state.toml`，然后丢掉旧注册。
3. 解析失败、已被占用、或写盘失败：回到旧热键，不留下只在内存里的新组合，底栏说明原因。

### 5.3 通知

写入 `state.toml` 后立刻翻转进程内开关。`PanelNotifier`（以及无界面的 `ConsoleNotifier`）始终 `log`；仅当开关为开且当前进程是面板时，再弹系统通知。面板状态行的 `notice` 事件不受此开关影响。

通知文案即现有 `Notifier::notify` 的中文消息（未能切换、唤醒失败、热键注册失败等）。不按事件再拆开关。用户可见文案不使用「交棒 / 持棒 / 接棒」。

系统通知失败（权限拒绝、API 不可用）只记日志，不得打断交棒，也不得把开关弹回（用户选择仍保存）。macOS 的通知调用放在 `baton-macos-sys/native`，Rust 侧只做 FFI；Windows 走托盘通知。不引入 Tauri 通知插件。

### 5.4 开机自启

只由 `deskbaton-gui` 管理，不进 `baton-core::ports`，不进任何 toml（`autostart_seeded` 除外）。无界面 `deskbaton` 不种、不删登录项。

| 平台 | 做法 |
|---|---|
| macOS | `~/Library/LaunchAgents/com.deskbaton.gui.plist`，`ProgramArguments` 为当前可执行文件的绝对路径；不设 `KeepAlive`（用户退出后面板不再被拉起，直到下次登录）。写入后 `launchctl bootstrap`，删除前 `bootout` |
| Windows | 当前用户 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 下值名 `DeskBaton`，数据为当前可执行文件的绝对路径 |

启动面板时：

1. `autostart_seeded` 为假：尝试开启登录项；成功则把 `autostart_seeded` 置真。失败不置位，下次启动再试，开关显示为关，底栏说明原因。
2. `autostart_seeded` 为真：只读取系统当前状态来显示开关。系统侧已关则保持关，不得再打开。系统侧已开则把目标路径更新为本次启动的可执行文件，避免开发版路径残留。

开关为开 = 已有我们写的那条登录项（macOS 那份 plist / Windows 那个值名），不论路径是否已过期；读到开时顺便把路径改成当前可执行文件。

用户拨动开关：开则写入登录项，关则删除（macOS 先 `bootout` 再删 plist）。失败则开关弹回，底栏说明原因。

## 6. 代码组织

```text
baton-core/src/state_file.rs   LocalState 新字段；读-改-写；名称/热键/通知与 Config 的合并（纯函数，可单测）
baton-agent                    启动时用合并后的名称和热键；学习/重新识别改为读-改-写
baton-net 发现与链路           运行中更新本机名称并重注册 mDNS
baton-gui/src/startup.rs       登录项，按 cfg(target_os) 分模块
baton-gui 通知                 PanelNotifier 读开关；macOS FFI / Windows 托盘
baton-gui/ui                   「本机」卡片
baton-macos-sys/native         仅当 macOS 系统通知需要 ObjC 时新增
```

不把开机自启做成 `baton-core` 端口：它只服务面板进程，不是交棒能力。

## 7. 测试

- **单测**（`state_file` / 合并函数 / 开机自启策略，平台 API 用假实现）：
  - 新字段缺省值、未知字段拒绝、读-改-写不丢其它字段
  - 名称：state 有值 / state 为空字符串 / 仅 config / 都缺
  - 热键：state > config > 默认
  - 通知缺省为开
  - `autostart_seeded` 为假时启动会尝试开启；为真且系统已关时不得再开；为真且系统已开时更新路径
  - 「重新识别」清掉 `input_code` 后，名称和热键仍在
- **CI**：既有 `fmt` / `clippy -D warnings` / `test`；Linux 继续排除 `baton-gui`
- **手动验收**：
  1. 第一次打开面板后重启系统，登录后面板自动起来；关掉开机自启再重启，不再起来
  2. 在系统登录项里关掉后，再开面板，开关为关且不会自己打开
  3. 改热键后不用退出，新组合能交棒，旧组合无效；故意设一个被占用的组合，原热键仍在
  4. 改名称后本机卡片和标题立刻变；对端下次握手后档案名称更新
  5. 关掉通知，制造一次交棒失败：只有日志和面板底栏，没有系统通知；打开通知后同性能弹系统通知

## 8. 风险

| 风险 | 应对 |
|---|---|
| 学习线程覆盖整个 `state.toml`，偏好丢失 | 统一读-改-写；单测「重新识别不丢名称」 |
| 用户在系统设置关掉开机自启后被再次打开 | `autostart_seeded` 为真后不再自动开启 |
| 开发版路径写进登录项 | 登录项已开启时每次启动刷新为当前可执行文件 |
| macOS 通知要授权 | 失败只记日志，不打断交棒，不改开关 |
| 改名后 mDNS 重注册失败 | 本机标题已改；底栏说明；不回滚已写入的名称 |
| 热键录制误触发交棒 | 录制期间丢弃这次按键，不送 `Event::Hotkey` |
| 手写 `config.toml` 与面板不一致 | 文档与本设计写明：面板改过的字段以 `state.toml` 为准 |
