# DeskBaton 显示器面板设计（首期：输入源切换 + 显示器信息）

日期：2026-10-16　状态：首期已实现，macOS 已验收（Windows 待验收）

`2026-10-17-zero-config-design.md` 把交棒显示器上的「输入源」卡片改为按电脑显示的「电脑」卡片，点对端即交棒；其它显示器保持本文的接口按钮。`2026-10-17-panel-settings-design.md` 在「设备」和底栏之间增加「本机」卡片。`2026-10-18-panel-platform-chrome-design.md` 覆盖第 7 节窗口形态 / 摆放 / 动效的 Windows 部分，并把 7.1 色彩表升级为共享 token。冲突时以它们为准。

## 1. 目标

做一个菜单栏（Windows 为系统托盘）弹出面板，用来查看外接显示器的基本信息并切换输入源。界面参照「Mi Monitor」类面板：标题栏、分辨率预览、输入源按钮。

## 2. 范围

**首期做**

- 列出支持 DDC/CI 的外接显示器，标题栏「⋯」菜单里切换当前控制的显示器
- 显示器信息：厂商 + 型号、当前输入源、分辨率、刷新率；macOS 另有渲染分辨率与是否 HiDPI，Windows 另有系统缩放比例。序列号不在状态行展示（P275MV 等机型 EDID 序列号全零，无辨识度），只在「⋯」菜单用末 6 位区分同型号多台
- 输入源按钮：按配置列出，高亮当前输入源，点击后经 DDC 切换
- macOS 与 Windows 同一套代码

**首期不做**

- 亮度、音量、场景及截图中的其它分页（不放占位）
- 读取显示器 capabilities 字符串自动识别输入口（后续再加，见 `TODO.md`）
- 与 agent 联动：面板切输入源不经交棒流程

## 3. 与 agent 的关系

面板直接调用 `baton_display::platform_display()` 切输入源，不经 `baton-core` 的交棒流程。

2026-10-17 起 agent 并入面板进程（`2026-10-17-pairing-design.md` 第 9 节）：`deskbaton-gui` 启动时调用 `baton_agent::Agent::start`，热键、长连接、配对都在面板进程里；无界面的 `deskbaton` 保留供调试，两者不同时运行。

代价：面板切走输入源时，协调器和对端不会知道，持棒状态可能与实际画面不一致。「切到对端机器的输入口」改为触发交棒另列 `TODO.md`。

为了便于以后并入，面板的后端逻辑（显示器视图组装、输入源名称查找）放在不依赖 Tauri 的模块里。

## 4. 代码组织

```text
crates/
  baton-gui/                 Tauri 2 应用，可执行文件 deskbaton-gui
    src/main.rs              Tauri 启动、托盘图标、显示 / 隐藏面板；退出时等 agent 结束
    src/agent.rs             进程内 agent（baton_agent::Agent）与热键注册
    src/commands.rs          Tauri 命令：list_displays / switch_input / fit_panel / quit / app_version
    src/devices.rs           Tauri 命令：agent_status 及设备配对（见配对设计第 10 节）
    src/view.rs              显示器视图组装、输入源名称查找、副标题拼接（纯逻辑，可单测）
    src/placement.rs         面板相对托盘图标的摆放计算（纯逻辑，可单测）
    tauri.conf.json
    ui/                      纯 HTML / CSS / JS（// @ts-check），不走 npm 打包
    ui/fonts/                Martian Mono Latin 子集 woff2 + OFL.txt
  baton-display/
    src/screen.rs            ScreenMode + screen_mode(&DisplayId)，按平台分模块
  baton-macos-sys/
    native/screen.m          CoreGraphics 读取当前显示模式
```

- Tauri 2 为 MIT / Apache-2.0，兼容 GPL-3.0；它要求 Rust 1.90，workspace 的 `rust-version` 随之提高。
- Tauri 不跟踪 `ui/` 的改动，`build.rs` 自行声明 `rerun-if-changed=ui`。
- `ScreenMode` 只供面板使用，作为 `baton-display` 的公开函数提供，不进入 `baton-core::ports`。

## 5. 后端

### 5.1 Tauri 命令

```rust
#[tauri::command]
async fn list_displays() -> Result<Vec<DisplayView>, String>;

#[tauri::command]
async fn switch_input(id: String, code: u8) -> Result<SwitchOutcome, String>;
```

```rust
struct DisplayView {
    id: String,
    name: String,              // 厂商 + 型号
    serial: Option<String>,
    screen: Option<ScreenMode>,
    current_input: Option<u8>, // 读不到时为 None（通常是本机不是活跃输入）
    inputs: Vec<InputButton>,  // 配置列表；当前编码不在列表里时追加一项 0xNN
    preferred: bool,           // display.match 命中的那台，作为默认选中
}

struct InputButton {
    label: String,
    code: u8,
    active: bool,
    owners: Vec<PortOwner>,    // 接在这个口上的电脑，可能多台（显示器把两个口报成同一编码）
}

struct PortOwner { name: String, platform: Option<Platform>, local: bool }

enum SwitchOutcome {
    Confirmed(u8), // 切换后回读成功
    LeftLocal,     // 写入成功但回读失败或不是目标编码：本机已不再持有画面，不算失败
}
```

**输入口上的设备**（`owners`）：

- 本机：`display.match` 命中的显示器上取配置的 `display.input_code`；其它显示器或未配置时取当前读到的编码（显示器只对活跃输入口回读请求，读得到即本机在用）。都没有时不标本机。平台取编译目标。
- 对端：已配对设备档案里的 `input_code` 与 `platform`（握手时上报，见配对设计第 4 节）。
- 编码不在配置列表里但有设备占用时，也追加一项 `0xNN`，保证设备都有地方显示。

- DDC 调用每次约 80 毫秒且会阻塞，命令内用 `spawn_blocking` 执行。
- 进程内用一把 `Mutex` 串行化所有 DDC 调用，避免并发 I2C 读写。
- 错误统一转为 `baton_core::Error` 的中文文案返回前端。

### 5.2 屏幕模式

```rust
pub struct ScreenMode {
    pub width: u32,          // macOS 为逻辑分辨率，Windows 为物理分辨率
    pub height: u32,
    pub pixel_width: u32,    // 渲染分辨率
    pub pixel_height: u32,
    pub refresh_hz: f64,
    pub scale_percent: Option<u32>, // Windows 的系统缩放比例；macOS 用逻辑分辨率表达缩放，为 None
}
// HiDPI = pixel_width > width，只在 macOS 上成立

pub fn screen_mode(display: &DisplayId) -> Result<ScreenMode>;
```

| 平台 | 实现 |
|---|---|
| macOS | `native/screen.m` 用 `CGGetActiveDisplayList` + `CGDisplayCopyDisplayMode` 读模式。CoreGraphics 显示器与 DDC 枚举到的 framebuffer 按厂商号、产品号、序列号（`CGDisplayVendorNumber` / `ModelNumber` / `SerialNumber` 对应 framebuffer 的 `ProductAttributes`）配对 |
| Windows | `DisplayId` 前缀即 `\\.\DISPLAYn`，用 `EnumDisplaySettingsW` 取物理分辨率，`GetDpiForMonitor` 取缩放比例，与「设置 → 显示」的写法一致。读 DPI 时把当前线程临时切到按显示器感知 DPI，否则未声明 DPI 感知的进程（如 CLI）只能读到 96 |
| 其它 | `Error::Unsupported` |

读不到屏幕模式时：若本进程曾经成功读过该显示器，继续显示上次的分辨率；否则预览卡片显示「未知」。不影响输入源切换。接棒后 IOKit 往往比 CoreGraphics 更早列出显示器，这时会读失败——用缓存顶上，并在约 800ms 后再读一次。配对不唯一（同型号且序列号相同）时按遍历顺序取，并记日志。

## 6. 配置

在 `[display]` 下新增 `inputs`，有默认值，旧配置文件无需修改：

```toml
[[display.inputs]]
label = "DP"
code = 0x0F

[[display.inputs]]
label = "HDMI1"
code = 0x11
```

默认列表：DP `0x0F`、HDMI1 `0x11`、HDMI2 `0x12`、USB-C `0x1B`。这些是 MCCS 常见值，不同显示器可能不同，需用户核对。例如 P275MV 把 USB-C 报成 `0x0F`，用默认列表时 USB-C 接入的 Mac 会显示为「DP」，须在配置里改为 `label = "USB-C"`、`code = 0x0F`（见 `config.example.toml`）。

面板复用 agent 的配置文件路径，只使用 `display` 段；文件不存在时用默认配置，解析失败时在底部状态行显示原因并使用默认配置。

## 7. 界面

```text
┌──────────────────────────────────┐
│ [图标] HP D27k            ⋯      │  ⋯：切换显示器 / 刷新 / 退出
│ 外接显示器 · DP · 2560×1440 · 60Hz│
├──────────────────────────────────┤
│        2560 × 1440               │
│  HiDPI · 渲染 5120×2880 · 60 Hz  │  Windows：缩放 175% · 144 Hz（100% 时省略）
│ ● 当前画面                  DP   │
├──────────────────────────────────┤
│ 输入源                           │
│ [USB-C ] [HDMI1] [HDMI2] [DP   ] │
│ [ 本机 ]               [ win-pc] │  接口名下一行：系统标识 + 设备名
│ [macOS ]               [Windows] │
├──────────────────────────────────┤
│ ● DDC 正常                       │  出错时显示中文原因
└──────────────────────────────────┘
```

- 打开面板时刷新一次；本机或已配对设备的持棒状态落定（变为持棒或空闲）时也刷新，短时间内的多次变化合并为一次，正在读取时顺延。切换期间按钮置灰，防止连点。
- `current_input` 为 `None` 时，「当前画面」显示「其它输入（本机读不到）」。
- 切换结果为 `LeftLocal` 时提示「已切到 XX，画面已离开本机」。

**窗口形态**：macOS 设为 Accessory（不在 Dock 显示）；点托盘图标显示无边框、置顶、透明背景的面板，失焦即隐藏。

- 摆放：每次按托盘图标**当前**矩形（`TrayIcon::rect`；读不到时才用上次点击记下的位置），水平居中并限制在图标所在屏幕内，与边缘留 6pt；图标下方放得下就放下方（macOS 菜单栏），否则放上方（Windows 底部任务栏）。当前矩形落不到任何屏幕（外接屏刚接回、坐标空间已变）时退到主屏，不记警告；连一块屏都没有时跳过这次摆放。不用 `tauri-plugin-positioner`：它在 macOS 上把面板顶边对齐图标顶边，会被菜单栏盖住，且不处理屏幕右缘。
- 点托盘图标关闭面板时，面板先失焦隐藏、随后才收到点击；隐藏后 300ms 内的点击不再重新打开。
- 高度随内容：前端渲染后调用 `fit_panel` 调整窗口高度并重新摆放。

### 7.1 视觉风格：信号台

样张：`assets/2026-10-16-display-panel-styles.html` 中的 A（B、C 为未采用的对照方案）。

**概念**：像显示器自带的 OSD 菜单——深石墨机身、等宽数字、一盏橙色信号灯指出「画面此刻从哪个口进来」。输入源按钮画成显示器背板上的接口轮廓，这是面板最有辨识度的元素。

**始终深色**，不跟随系统浅色模式：面板是覆盖在桌面上的仪表，深色在任何壁纸上都稳定。

**色彩**（CSS 变量）

| 变量 | 值 | 用途 |
|---|---|---|
| `--bg` | `#121315` | 面板底色，叠 6% 不透明度的噪点纹理 |
| `--card` | `#1A1B1E` | 卡片底色 |
| `--line` | `rgba(255,255,255,.07)` | 1px 描边、分隔 |
| `--ink` | `#ECEAE6` | 主文字 |
| `--mute` | `#8A8781` | 次要文字、未选中接口 |
| `--signal` | `#FF6A1A` | 唯一强调色：当前输入源、信号灯、分辨率中的 `×` |
| `--signal-dim` | `rgba(255,106,26,.16)` | 选中接口底色 |
| `--ok` | `#3DDC84` | 状态行「DDC 正常」 |
| `--err` | `#FF5A4E` | 状态行错误 |

橙色只用在「当前信号」相关的元素上，不做装饰。

**字体**

- 数字、接口名、技术参数（分辨率、`VCP 0x60`、版本号）：Martian Mono（SIL OFL 1.1），只打包 Latin 子集的 woff2，放 `ui/fonts/`，附带 `OFL.txt`；不从网络加载。
- 中文与普通文字：系统字体，`-apple-system, "PingFang SC", "Microsoft YaHei", sans-serif`。
- 字号：分辨率 26px / 600；显示器名称 17px / 600；卡片标题 13px / 600；接口名 10px / 600；参数行 9.5–10px。

**尺寸**：面板宽 360px，高随内容；外圆角 18px，卡片圆角 12px，接口按钮圆角 9px；外边距 16px，卡片间距 10px。

**组件**

- 标题栏：36px 橙色渐变图标块 + 名称 + 等宽副标题；右侧 30px「⋯」描边按钮。
- 预览卡片：16:9 屏幕框（3px 深灰边框），屏内为自上而下的橙色径向辉光 + 极淡扫描线，中央是分辨率；下方底座短横线；最下一行「● 当前画面 …… DP」，信号灯带发光。
- 输入源：一行四格；每格为接口轮廓图标（DP 缺角矩形、HDMI 梯形、USB-C 胶囊、未知编码用普通矩形）+ 名称 + 右上角 4px 指示灯。有设备占用时名称下方再加两行 9px 小字：设备名（本机写「本机」，过长省略，完整名放 `title`）和系统名（macOS / Windows / Linux，未知不写）；多台设备时设备名用「、」连接。没有设备的格子不加这两行，各格顶端对齐。
- 「当前画面」一行右侧：接口名后接占用设备，如「USB-C · 本机」。
- 状态行：左侧状态点 + 文案，右侧等宽应用版本号（`v` + workspace `version`，如 `v0.0.1`）。

**状态**

| 状态 | 表现 |
|---|---|
| 当前输入源 | 文字和轮廓变 `--signal`，轮廓填充 25% 橙色，底色 `--signal-dim`，指示灯点亮发光 |
| 悬停 | 文字变 `--ink`，描边变亮 |
| 切换中 | 目标接口指示灯闪烁（1s 周期），其它接口置灰不可点 |
| 本机不再持有画面（`LeftLocal` 或读不到当前输入） | 屏幕辉光熄灭，「当前画面」显示「其它输入」且信号灯变灰；被切到的接口保持高亮并标注「已切到」 |
| 屏幕模式未知 | 屏内显示「未知分辨率」，副标题省略分辨率和刷新率 |
| DDC 出错 | 状态点 `--err`，文案为中文错误原因 |

**动效**：打开面板时各卡片依次上浮淡入（每块间隔 60ms，0.5s）；接口切换用 0.2s 颜色过渡，按下缩放到 0.97。不做其它装饰性动画。

## 8. 测试

- 单测（`view.rs`、`baton-core` 配置）：
  - 输入源名称查找：命中配置、编码不在列表时追加 `0xNN`、`current_input` 为 `None`
  - 输入口上的设备：本机取配置编码或当前编码、对端按档案编码、两台设备同一编码、设备编码不在列表时追加
  - 副标题拼接：有 / 无屏幕模式、HiDPI 判定
  - `display.inputs` 解析：缺省值、自定义列表、未知字段报错
  - 面板摆放：菜单栏图标下方居中、屏幕右缘限制、底部任务栏图标上方、副屏坐标偏移、托盘点不在任何屏上时退到第一块屏
  - 屏幕模式缓存：读到则更新、读失败用上次的、从未读到过则为空
- CI：Linux 不是目标平台且 Tauri 依赖 webkit2gtk，Linux 任务排除 `baton-gui`。
- 硬件验收清单（手动）：
  1. 在接了两台 HP D27k 的 Mac mini 上，面板能列出两台并在「⋯」中切换；
  2. 显示的型号、当前输入源与 `deskbaton-cli displays / get-input` 一致；分辨率、刷新率与「系统设置 → 显示器」一致；状态行右侧为应用版本号；
  3. 点击另一个输入源按钮，显示器切走，面板提示本机不再持有画面；切回后刷新，高亮正确；
  4. Windows 上重复第 1–3 项。

**验收记录（2026-10-16，Mac mini M4 Pro + 两台 HP D27k，其中一台竖屏）**

- 第 1–3 项通过：面板在菜单栏图标正下方弹出，再点图标或点别处收起；两台显示器信息正确，竖屏显示为竖向屏幕框；切到其它输入源后显示器切走，切回后高亮正确。
- 首版摆放错误：面板弹到另一台显示器左侧。原因是 macOS 上 `monitor_from_point` 要求全局逻辑坐标，而托盘矩形是物理坐标；已改为按各屏缩放比例换算后再匹配屏幕（见第 7 节「摆放」）。
- 屏幕模式读取与 `deskbaton-cli displays` 一致：1920×1080（渲染 3840×2160）60Hz、1080×1920（渲染 2160×3840）60Hz。

## 9. 风险

| 风险 | 应对 |
|---|---|
| CoreGraphics 与 framebuffer 配对失败 | 屏幕模式显示「未知」，不影响切换；记日志便于排查 |
| 面板切输入源导致持棒状态不一致 | agent 已并入面板进程；面板切输入源改走交棒另列 TODO |
| Tauri 依赖多、首次编译慢 | 只在 `baton-gui` 引入，不影响其它 crate |
