# DeskBaton 显示器亮度与音量设计

日期：2026-10-27　状态：已实现，待硬件验收

调研依据：`docs/research/2026-10-27-brightness-volume-landscape.md`。本文扩展 `2026-10-16-display-panel-design.md`：在「显示器」分页的输入源卡片下方加「亮度与声音」卡片。两文冲突时，以本文为准。

## 1. 目标

画面在本机时，用户可以在面板里调当前显示器的亮度、音量，开关静音。优先经 DDC 调显示器本身；显示器明确不支持时，亮度改用软件调光，音量在 Windows 上改调系统音量。

## 2. 范围

**做**

- 当前选中显示器的亮度滑条、音量滑条、静音开关
- 控制路径：

  | 项目 | 首选 | 兜底 | 都不行 |
  |---|---|---|---|
  | 亮度 | DDC `0x10` | 软件调光（gamma） | 显示「这台显示器不支持调亮度」 |
  | 音量 | DDC `0x62` | Windows 系统音量 | 显示「这台显示器不支持调音量」 |
  | 静音 | DDC `0x8D` | 随音量走系统音量的静音 | 不显示静音开关 |

- macOS 与 Windows 用同一套面板代码

**不做**

- macOS 系统音量：显示器的 DP / HDMI 音频设备在 macOS 上没有音量和静音属性（调研第 3 节实测），要调只能装虚拟声卡
- 热键、系统媒体键
- 对比度和其它 VCP
- DDC 亮度到底后再叠加 gamma 继续调暗（MonitorControl 的「组合亮度」）
- 遮罩窗口调光
- 经网络调对端显示器

## 3. 不变量

- **只在本机持有画面时调节**：`current_input` 为 `None`（本机读不到当前输入源）时，卡片整体置灰，提示「画面不在本机，无法调节」。这与「只有持棒方发 DDC」一致。
- **不走网络**：不涉及交棒流程，也不改线上格式，`PROTOCOL_VERSION` 不变。
- **软件调光不留残留**：
  - 不持久化调光值。
  - 亮度回到 100% 时立即复原 gamma，面板退出时也复原。
  - 上次异常退出没复原的，下次启动时复原（第 5.4 节）。
  - 本条同时写进 `AGENTS.md`「产品不变量」。
- **区分「不支持」和失败**：只有显示器明确回「不支持」，才改用兜底。I/O 失败、回空消息只报错，不切换路径，也不缓存结果。

## 4. 代码组织

```text
crates/
  baton-display/
    src/levels.rs       Levels 状态、路径选择、百分比换算（纯逻辑，用假后端单测）
    src/macos.rs        Get / Set VCP 通用化；gamma 经 baton-macos-sys
    src/windows.rs      Get / Set VCP 通用化；gamma（SetDeviceGammaRamp）；端点音量
    src/mccs.rs         新增 VCP 常量
  baton-macos-sys/
    native/gamma.m      CGGetDisplayTransferByTable / CGSetDisplayTransferByTable / CGDisplayRestoreColorSyncSettings
  baton-core/
    src/config.rs       display.audio_device
    src/state_file.rs   dimmed_displays
  baton-gui/
    src/commands.rs     read_levels / set_level
    ui/                 「亮度与声音」卡片
```

`ScreenMode` 只供面板使用，所以放在 `baton-display`、不进 `ports`；亮度、音量也一样，作为 `baton-display` 的公开 API 提供。

## 5. 后端

### 5.1 公开 API

```rust
pub enum Source { Ddc, Gamma, System }

pub struct Slider { pub percent: u8, pub source: Source }

pub struct LevelsView {
    pub brightness: Option<Slider>, // None：DDC 不支持，且读不到这台显示器的 gamma 表
    pub volume: Option<Slider>,     // None：显示「不支持调音量」
    pub muted: Option<bool>,        // None：不显示静音开关
}

pub enum Control { Brightness, Volume, Mute }

/// 面板进程内唯一一份，持有路径判定缓存和 gamma 基准表。调用方负责串行化（复用面板的 DDC 锁）。
pub struct Levels { /* ... */ }

impl Levels {
    pub fn new(audio_device: Option<String>) -> Self;
    pub fn read(&mut self, display: &DisplayInfo) -> Result<LevelsView>;
    /// Mute 时 value 为 0 / 1。
    pub fn set(&mut self, display: &DisplayInfo, control: Control, value: u8) -> Result<()>;
    /// 「⋯ → 刷新」时调用，下次 read 重新判定路径。
    pub fn forget(&mut self);
    /// 复原本进程调过的所有 gamma。
    pub fn restore_all(&mut self);
}

/// 启动时复原上次残留的软件调光。
pub fn restore_dimmed(displays: &[DisplayId]);
```

平台差异集中在 `levels.rs` 内部的后端 trait：读写 VCP、读写 gamma、读写端点音量。单测用假后端。

### 5.2 DDC

- **报文**：沿用 `mccs.rs`，新增 `VCP_BRIGHTNESS = 0x10`、`VCP_VOLUME = 0x62`、`VCP_MUTE = 0x8D`。macOS 的 `get_input` 里那段收发逻辑抽成通用的 `get_vcp(index, code) -> VcpRead`；Windows 同理。
- **判定「不支持」**：
  - macOS：回包结果码（第 4 字节）非 0。
  - Windows：`GetVCPFeatureAndVCPFeatureReply` 失败，且错误码为 `ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED`（`0xC0262584`）。
  - 判定为不支持时直接返回，不走 `retry`。
- **换算**：最大值取 `min(回包最大值, 100)`，回包最大值为 0 时按 100 算。百分比和原始值互转时四舍五入。音量只要大于 0%，至少写 1；0% 时写 0（调研第 1 节）。
- **静音**：`1` 表示静音，`2` 表示取消静音。读到其它值按未静音处理。
- **路径缓存**：每台显示器的每一项，第一次 `read` 时判定，结果缓存在 `Levels` 里，直到 `forget`。

### 5.3 软件调光（gamma）

只在 DDC 亮度明确不支持时使用。

**macOS**（照 MonitorControl）

- 第一次调暗时，用 `CGGetDisplayTransferByTable` 读 256 项表作为基准。
- 设为 `p%` 时，三个通道都乘 `0.15 + 0.85 × p / 100` 再写回，所以 0% 对应 15%。
- CoreGraphics 显示器沿用 `screen.m` 的配对方式（厂商号、型号、序列号）。

**Windows**（照 twinkle-tray）

- 第一次调暗时，用 `GetDeviceGammaRamp` 读 `\\.\DISPLAYn` 的表作为基准。同一个 `DISPLAYn` 下的多台物理显示器共用这张表。
- 有效亮度 `20% + 0.8 × p`，按 twinkle-tray 的曲线生成新表：低于 70% 时峰值固定在 70%，只压中间调，避开 GDI 的静默拒绝。
- 写入后回读，误差在 256 以内才算成功，最多 3 次。仍不成功时报错「软件调光没有生效（HDR 下不可用）」。

**两端共同**

- **当前值**：每次 `read` 回读当前表。如果它已经不是「基准表套上我们最后一次写的系数」，就认为被系统或其它应用重置了（显示器重配置、交棒切回、f.lux 等）：显示 100%，丢掉基准表，不自动重新应用。
- **复原**：设回 100% 时写回基准表，并丢掉它。

### 5.4 残留恢复

- `state.toml` 新增 `dimmed_displays: Vec<String>`（默认空），记录当前调暗中的 `DisplayId`。调到低于 100% 时加入，复原时移除。
- **正常退出**：`RunEvent::Exit` 时调 `restore_all`，然后清空 `dimmed_displays`。
- **启动**：`dimmed_displays` 非空时调 `restore_dimmed`，然后清空。
  - macOS 调一次 `CGDisplayRestoreColorSyncSettings`，复原所有显示器。只在有残留时才调，以免平时冲掉 f.lux 的设置。
  - Windows 对记录里的每个 `DISPLAYn` 写恒等表。之后系统的校准加载器可能会再改，不影响。

### 5.5 Windows 系统音量

只在 DDC 音量明确不支持时使用。

- **找端点**：
  - 用 `IMMDeviceEnumerator::EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)` 列出端点，读 `PKEY_Device_FriendlyName`。
  - 名称包含匹配串（不区分大小写）的端点，就是这台显示器的音频。匹配串取配置 `display.audio_device`，没配置时取 EDID 型号（`DisplayInfo.model`）。
  - 恰好一个匹配才用；零个或多个时按不支持处理，并记日志。
- **读写**：`IAudioEndpointVolume::GetMasterVolumeLevelScalar` / `SetMasterVolumeLevelScalar`，静音用 `GetMute` / `SetMute`。
- **COM**：在阻塞线程上 `CoInitializeEx(COINIT_MULTITHREADED)`，用完 `CoUninitialize`。
- 端点名称的实际格式未验证，列入验收清单。

### 5.6 Tauri 命令与写入合并

```rust
/// fresh：先 forget，重新判定路径（「⋯ → 刷新」）。
#[tauri::command]
async fn read_levels(id: String, fresh: bool) -> Result<LevelsView, String>;

/// 返回这次是否真的下发。
#[tauri::command]
async fn set_level(id: String, control: Control, value: u8) -> Result<bool, String>;
```

- 两个命令都在 `spawn_blocking` 里执行，并先拿面板现有的 DDC 锁。`Levels` 放在 `Panel` 里。
- **写入合并**：`set_level` 先把值写进 `pending[(id, control)]`，再去拿锁；拿到锁后取出最新值写入。如果已经被别的调用取走，返回 `false`。这样拖动时排队的旧值不会写到显示器上；前端只把返回 `true` 的值记为「上次成功的值」。
- `read` 与 `set` 前后调暗的显示器有变化时，立即写回 `dimmed_displays`。
- `list_displays` 不读亮度和音量，保证主视图的刷新时长不变。

## 6. 界面

```text
├──────────────────────────────────┤
│ 亮度与声音                         │
│ 亮度  [━━━━━━━━●──────]  62  DDC  │
│ 音量  [━━━●───────────]  30  DDC  │  [静音]
├──────────────────────────────────┤
```

- **加载**：主视图渲染完成后，前端对当前选中的显示器调用 `read_levels`，读取期间卡片显示「正在读取…」。切换显示器、交棒状态落定刷新、「⋯ → 刷新」时重读；「⋯ → 刷新」还会先 `forget`。
- **来源标注**：每行右侧用等宽小字标出来源：`DDC` / `软件调光` / `系统音量`。
- **不支持**：音量不支持时，这一行换成「这台显示器不支持调音量」；静音为 `None` 时不显示静音开关。亮度为 `None` 时，这一行换成「这台显示器不支持调亮度」。`read_levels` 本身失败（I/O 错误等）时，卡片显示错误原因，滑条不可用。
- **画面不在本机**：卡片置灰，只显示一行「画面不在本机，无法调节」，不调 `read_levels`。
- **拖动**：滑条拖动期间最多每 100 ms 发一次 `set_level`，松手时一定再发一次。失败时滑条退回上次成功的值，状态行显示原因。
- **样式**：沿用 7.1 节的色彩 token。滑条轨道用 `--bezel`，已填充部分和滑块用 `--ink`；`--signal` 仍然只用于当前信号相关的元素，不用在滑条上。静音开关用 `.btn ghost`，静音中改为 `.btn`。

## 7. 配置

`[display]` 新增一项，有默认值，旧配置文件不用改：

```toml
[display]
# Windows 按这个子串找显示器的音频端点；不写时用 EDID 型号
# audio_device = "HP D27k"
```

## 8. 测试

**单测**（`levels.rs` 的假后端、`baton-core` 配置与状态）

- 路径选择：
  - DDC 正常时用 DDC。
  - 明确不支持时亮度改 gamma、音量改系统音量。
  - I/O 失败只报错，不切换路径，也不缓存。
  - 系统音量找不到唯一端点时，音量为 `None`。
  - 静音的三种来源。
- 换算：最大值截到 100、最大值为 0、音量大于 0% 时至少写 1、四舍五入往返。
- gamma：
  - 系数映射（macOS 下限 15%、Windows 曲线）。
  - 回读到的表与最后一次写入不符时显示 100%，并丢掉基准表。
  - 设回 100% 时写回基准表。
- `dimmed_displays`：加入、移除、启动时复原后清空。
- 配置：`display.audio_device` 的缺省值、自定义值、未知字段报错。

**硬件验收清单**（手动）

1. Mac mini + HP D27k：亮度显示为 `DDC`，拖动时显示器亮度跟着变，面板关掉再开，数值一致；音量一行显示「不支持调音量」，没有静音开关。
2. 交棒到对端后，本机面板的卡片置灰、提示画面不在本机；切回后数值正确。
3. 找一台不支持 DDC 亮度的显示器（或在显示器 OSD 里关掉 DDC/CI）：亮度显示为 `软件调光`，0% 时屏幕明显变暗但仍可读；退出面板后亮度复原；强制结束进程后重新启动，亮度复原。
4. Windows：重复第 1–3 项；HDR 开着时软件调光报错，不卡死。
5. Windows + 支持 DDC 音量的显示器：音量显示为 `DDC`，静音开关生效。
6. Windows + 不支持 DDC 音量、但经 HDMI / DP 出声的显示器：音量显示为 `系统音量`，与系统托盘里的音量一致；端点名称记录到调研文档；名称匹配不到时，配置 `audio_device` 后生效。

## 9. 风险

| 风险 | 应对 |
|---|---|
| gamma 与 f.lux、夜览、校准工具冲突 | 只在 DDC 不支持时用；发现被改就显示 100% 并放手，不和别人抢 |
| Windows 静默拒绝过暗的表 | 采用 twinkle-tray 的曲线和下限，写后回读校验 |
| 异常退出后屏幕一直偏暗 | 用 `dimmed_displays` 标记，下次启动复原 |
| 显示器端点名称和 EDID 型号不一致 | 提供 `display.audio_device` 覆盖；验收时记录实际名称 |
| 部分显示器把 VCP 不支持报成 I/O 失败 | 不会误切兜底，只显示错误；遇到实例时按型号另行处理 |
