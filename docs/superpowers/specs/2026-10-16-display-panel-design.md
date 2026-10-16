# DeskBaton 显示器面板设计（首期：输入源切换 + 显示器信息）

日期：2026-10-16　状态：已确认，待实现

## 1. 目标

做一个菜单栏（Windows 为系统托盘）弹出面板，用来查看外接显示器的基本信息并切换输入源。界面参照「Mi Monitor」类面板：标题栏、分辨率预览、输入源按钮。

## 2. 范围

**首期做**

- 列出支持 DDC/CI 的外接显示器，标题栏「⋯」菜单里切换当前控制的显示器
- 显示器信息：厂商 + 型号、序列号、当前输入源、分辨率、渲染分辨率、刷新率、是否 HiDPI
- 输入源按钮：按配置列出，高亮当前输入源，点击后经 DDC 切换
- macOS 与 Windows 同一套代码

**首期不做**

- 亮度、音量、场景及截图中的其它分页（不放占位）
- 读取显示器 capabilities 字符串自动识别输入口（后续再加，见 `TODO.md`）
- 与 agent 联动：面板切输入源不经交棒流程

## 3. 与 agent 的关系

面板是独立进程，直接调用 `baton_display::platform_display()`，不连接 agent，不改 `baton-core` 的交棒流程。

代价：agent 同时运行时，面板切走输入源，对端 agent 不会知道，持棒状态可能与实际画面不一致。首期接受这一点；M2 做托盘时再把面板并入 agent，届时「切到对端机器的输入口」改为触发交棒。

为了便于以后并入，面板的后端逻辑（显示器视图组装、输入源名称查找）放在不依赖 Tauri 的模块里。

## 4. 代码组织

```text
crates/
  baton-gui/                 Tauri 2 应用，可执行文件 deskbaton-gui
    src/main.rs              Tauri 启动、托盘图标、窗口定位与失焦隐藏
    src/commands.rs          Tauri 命令：list_displays / switch_input
    src/view.rs              显示器视图组装、输入源名称查找、副标题拼接（纯逻辑，可单测）
    tauri.conf.json
    ui/                      纯 HTML / CSS / JS（// @ts-check），不走 npm 打包
  baton-display/
    src/screen.rs            ScreenMode + screen_mode(&DisplayId)，按平台分模块
  baton-macos-sys/
    native/screen.m          CoreGraphics 读取当前显示模式
```

- Tauri 2 及插件均为 MIT / Apache-2.0，兼容 GPL-3.0。
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

struct InputButton { label: String, code: u8, active: bool }

enum SwitchOutcome {
    Confirmed(u8), // 切换后回读成功
    LeftLocal,     // 写入成功但回读失败：本机已不再持有画面，不算失败
}
```

- DDC 调用每次约 80 毫秒且会阻塞，命令内用 `spawn_blocking` 执行。
- 进程内用一把 `Mutex` 串行化所有 DDC 调用，避免并发 I2C 读写。
- 错误统一转为 `baton_core::Error` 的中文文案返回前端。

### 5.2 屏幕模式

```rust
pub struct ScreenMode {
    pub width: u32,          // 逻辑分辨率
    pub height: u32,
    pub pixel_width: u32,    // 渲染分辨率
    pub pixel_height: u32,
    pub refresh_hz: f64,
}
// HiDPI = pixel_width > width

pub fn screen_mode(display: &DisplayId) -> Result<ScreenMode>;
```

| 平台 | 实现 |
|---|---|
| macOS | `native/screen.m` 用 `CGGetActiveDisplayList` + `CGDisplayCopyDisplayMode` 读模式。CoreGraphics 显示器与 DDC 枚举到的 framebuffer 按厂商号、产品号、序列号（`CGDisplayVendorNumber` / `ModelNumber` / `SerialNumber` 对应 framebuffer 的 `ProductAttributes`）配对 |
| Windows | `DisplayId` 前缀即 `\\.\DISPLAYn`，用 `EnumDisplaySettingsW` 取模式，`GetDpiForMonitor` 取缩放比例换算逻辑分辨率 |
| 其它 | `Error::Unsupported` |

读不到屏幕模式时预览卡片显示「未知」，不影响输入源切换。配对不唯一（同型号且序列号相同）时按遍历顺序取，并记日志。

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

默认列表：DP `0x0F`、HDMI1 `0x11`、HDMI2 `0x12`、USB-C `0x1B`。这些是 MCCS 常见值，不同显示器可能不同，需用户核对。

面板复用 agent 的配置文件路径，只使用 `display` 段；文件不存在时用默认配置，解析失败时在底部状态行显示原因并使用默认配置。

## 7. 界面

```text
┌──────────────────────────────────┐
│ [图标] HP D27k            ⋯      │  ⋯：切换显示器 / 刷新 / 退出
│ 外接显示器 · DP · 2560×1440 · 60Hz│
├──────────────────────────────────┤
│        2560 × 1440               │
│  HiDPI · 渲染 5120×2880 · 60 Hz  │
│ ● 当前画面                  DP   │
├──────────────────────────────────┤
│ 输入源                           │
│ [DP] [HDMI1] [HDMI2] [USB-C]     │
├──────────────────────────────────┤
│ ● DDC 正常                       │  出错时显示中文原因
└──────────────────────────────────┘
```

- 深色卡片风格，强调色橙色，当前输入源按钮高亮。
- 打开面板时刷新一次；切换期间按钮置灰，防止连点。
- `current_input` 为 `None` 时，「当前画面」显示「其它输入（本机读不到）」。
- 切换结果为 `LeftLocal` 时提示「已切到 XX，本机不再持有画面」。

**窗口形态**：macOS 设为 Accessory（不在 Dock 显示）；点托盘图标在图标正下方显示无边框、置顶的面板（`tauri-plugin-positioner`），失焦即隐藏。

## 8. 测试

- 单测（`view.rs`、`baton-core` 配置）：
  - 输入源名称查找：命中配置、编码不在列表时追加 `0xNN`、`current_input` 为 `None`
  - 副标题拼接：有 / 无屏幕模式、HiDPI 判定
  - `display.inputs` 解析：缺省值、自定义列表、未知字段报错
- CI：Linux 不是目标平台且 Tauri 依赖 webkit2gtk，Linux 任务排除 `baton-gui`。
- 硬件验收清单（手动）：
  1. 在接了两台 HP D27k 的 Mac mini 上，面板能列出两台并在「⋯」中切换；
  2. 显示的型号、序列号、当前输入源与 `deskbaton-cli displays / get-input` 一致；分辨率、刷新率与「系统设置 → 显示器」一致；
  3. 点击另一个输入源按钮，显示器切走，面板提示本机不再持有画面；切回后刷新，高亮正确；
  4. Windows 上重复第 1–3 项。

## 9. 风险

| 风险 | 应对 |
|---|---|
| CoreGraphics 与 framebuffer 配对失败 | 屏幕模式显示「未知」，不影响切换；记日志便于排查 |
| 面板与 agent 同时运行导致持棒状态不一致 | 首期接受；M2 并入 agent |
| Tauri 依赖多、首次编译慢 | 只在 `baton-gui` 引入，不影响其它 crate |
