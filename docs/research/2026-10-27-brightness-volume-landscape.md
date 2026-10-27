# 显示器亮度 / 音量控制调研（2026-10-27）

目的：给 DeskBaton 加显示器亮度、音量控制。主路径走 DDC，DDC 不可用时用软件调光、系统音量兜底。只读开源实现（许可证均兼容 GPL-3.0）和平台官方文档，不反编译闭源软件。

参考版本：

| 项目 | 平台 | 许可证 | 提交 |
|---|---|---|---|
| [MonitorControl/MonitorControl](https://github.com/MonitorControl/MonitorControl) | macOS | MIT | `7cfe645`（2026-10-14） |
| [emoacht/Monitorian](https://github.com/emoacht/Monitorian) | Windows | MIT | `60f4597`（2026-10-22） |
| [xanderfrangos/twinkle-tray](https://github.com/xanderfrangos/twinkle-tray) | Windows | MIT | `f4b36e1`（2026-10-21） |

BetterDisplay 3.5.5 是闭源软件，只用 `otool -L` 看了它链接的系统框架：IOKit（DDC）、私有的 DisplayServices / CoreBrightness（Apple 显示器原生亮度）、CoreDisplay / SkyLight（gamma 调光）、CoreAudio / AVFAudio、私有 OSD（系统浮层）。思路与下面的开源项目一致，不再深入。

## 结论

1. **DDC 编码**：亮度 `0x10`、对比度 `0x12`、音量 `0x62`、静音 `0x8D`（`1` 静音、`2` 取消）。现有 `mccs.rs` 的 Get / Set VCP 能直接用，回包里本来就带最大值。
2. **最大值不可信**：MonitorControl 把读到的最大值截到 100，读不到时也按 100 算。Monitorian 只用回包里的最大值，最小值按 0 算。
3. **拖动时要合并写入**：两边都是只保留最新值。MonitorControl 用串行队列，每次取最新值，和上次写入的值相同就跳过。twinkle-tray 按「显示器 + VCP」节流，默认 500 ms 写一次，中间值只保留最后一个。
4. **音量不写 0**：MonitorControl 在用户要求的音量大于 0 时至少写 1，源码注释说写 0 会让部分显示器坏掉。静音另走 `0x8D`。
5. **macOS 软件调光**：把启动时读到的 gamma 表整体乘一个系数（`CGSetDisplayTransferByTable`），下限 15%。显示器重配置时先调 `CGDisplayRestoreColorSyncSettings` 复原，过 1 秒再重新应用。还会检测 f.lux 之类的冲突。
6. **Windows 软件调光**：`SetDeviceGammaRamp`。微软文档明确不推荐：太暗的表会被「静默拒绝」（返回 TRUE 但不生效）；任何显示事件都会把表重置；HDR 下行为未定义；进程退出后表还留着。twinkle-tray 的对策：下限 20%；低于 70% 时峰值保持不动，只压中间调；写入后回读校验，最多 3 次；HDR 下禁用；退出时复原。
7. **系统音量兜底只在 Windows 上可行**。在 macOS 上实测，显示器的 DP/HDMI 音频设备没有音量和静音属性（见下文实测）；要调只能装虚拟声卡，超出范围。Windows 的 `IAudioEndpointVolume` 在硬件没有音量控制时由音频引擎做软件音量，可以兜底。
8. **哪路音频属于哪台显示器**：MonitorControl 拿 CoreAudio 设备名和显示器名比较，比较前去掉括号、空格和数字，并允许用户手填覆盖。本机实测设备名就是「HP D27k」。Windows 侧两个参考项目都不做系统音量，匹配规则得自己定，需实测。

## 1. DDC 读写

### 编码与取值

- MonitorControl `Enums/Command.swift`：`brightness = 0x10`、`contrast = 0x12`、`audioSpeakerVolume = 0x62`、`audioMuteScreenBlank = 0x8D`。
- Monitorian `MonitorConfiguration.cs` 第 187–191 行：`Luminance = 0x10`、`Contrast = 0x12`、`SpeakerVolume = 0x62`。
- 静音取值：MonitorControl `OtherDisplay.swift` `toggleMute()`（第 221–255 行）写 `1` 表示静音、`2` 表示取消静音；没读到过时按 `2` 处理（第 150 行）。

### 最大值与换算

- MonitorControl `main.swift`：`DDC_MAX_DETECT_LIMIT = 100`。`setupCurrentAndMaxValues`（`OtherDisplay.swift` 第 91–144 行）用 `min(读到的最大值, 100)`，读失败时最大值按 100 算。用户可以手动覆盖最小 / 最大值（`minDDCOverride` / `maxDDCOverride`）。
- 读不到当前值时的默认值：音量 12.5%（注释说是怕声音太大吓到用户）、对比度 75%、亮度 100%（第 95–102 行）。
- `convValueToDDC`（第 482–497 行）：内部值是 0–1 的浮点数，按 `(max - min) × value + min` 换算；音量在用户要求的值大于 0 时至少写 1。
- Monitorian `GetValue`（第 465–493 行）直接用 `GetVCPFeatureAndVCPFeatureReply` 回包里的最大值，最小值按 0 算。注释说原始亮度「不总是 0–100」，要按最小 / 最大值换算成百分比。

### 写入节奏

- MonitorControl `writeDDCValues` / `asyncPerformWriteDDCValues`（第 380–419 行）：先把值写进 `writeDDCNextValue[command]`，再到全局 DDC 队列里取最新值写入；和上次写入的值相同就跳过。所有显示器共用一个全局串行队列（`DisplayManager.globalDDCQueue`）。
- MonitorControl `Arm64DDC.swift` 第 92–114 行，默认节奏：每次写间隔 10 ms，连写 2 次；等 50 ms 后读回包；失败重试 4 次，间隔 20 ms。与 DeskBaton 现有 `ddc.m` 的写间隔 10 ms、连写 2 次、等 50 ms 一致。
- twinkle-tray `electron.js` 第 3050–3090 行 `updateBrightnessThrottle`：按「显示器 + VCP」分开节流，默认 `updateInterval: 500`（第 884 行）；节流期间只保留最新值，到点再写。
- MonitorControl 在睡眠和显示器重配置期间不发 DDC（`app.sleepID == 0, app.reconfigureID == 0` 守卫）；唤醒后等 3 秒（`AppDelegate.swift` 第 207–215 行），重配置后等 1 秒（第 146–149 行）。

### 「不支持」怎么判定

- MonitorControl 不自动判定，只提供一个用户开关（`unavailableDDC`）手动关掉某项 DDC 控制。
- Monitorian 先读 capabilities 字符串，再看里面有没有 `0x10`（第 246–288 行）。Windows 上 VCP 不支持时，`GetVCPFeatureAndVCPFeatureReply` 失败并返回 `ERROR_GRAPHICS_DDCCI_VCP_NOT_SUPPORTED`（`0xC0262584`）；校验和、长度类错误和 I2C 传输错误算瞬时错误，重试一次（第 571–602 行）。
- MCCS 回包第 4 字节（结果码）非 0 表示不支持。现有 `mccs::parse_vcp_reply` 已把它报成「显示器不支持 VCP」，但没有和其它错误区分开。

### 本机实测（Mac mini M4 Pro + 两台 HP D27k，2026-10-27）

用临时脚本只读 Get VCP（写 2 次、等 50 ms，与 `ddc.m` 相同）：

| 显示器 | `0x10` 亮度 | `0x12` 对比度 | `0x62` 音量 | `0x8D` 静音 |
|---|---|---|---|---|
| D27k #1 | 62 / 100 | 80 / 100 | 不支持 | 不支持 |
| D27k #2 | 59 / 100 | 80 / 100 | 不支持 | 不支持 |

## 2. macOS 软件调光

出处：MonitorControl `Model/Display.swift`、`Support/DisplayManager.swift`、`Support/AppDelegate.swift`。

- **基准表**：启动时用 `CGGetDisplayTransferByTable` 读 256 项 gamma 表作基准（`swUpdateDefaultGammaTable`，第 190–199 行）。
- **调光**：三个通道都乘系数 `v`，用 `CGSetDisplayTransferByTable` 写回（第 246–252 行）。`v` 映射到 `[0.15, 1]`，只有用户打开「允许软件亮度到 0」时下限才为 0（`swBrightnessTransform`，第 201–208 行）。
- **当前值**：读当前 gamma 表的峰值，除以基准表峰值（`getSwBrightness`，第 259–288 行）。
- **冲突**：发现 gamma 值和自己上次写的差了 0.02 以上，就判定有其它应用（f.lux 等）在改，先复原；累计 3 次后弹窗，让用户在「退出那个应用」和「改用遮罩窗口」之间选（`checkGammaInterference`，第 290–320 行）。
- **重配置**：收到 ColorSync 显示配置通知后，先把软件亮度复原、调 `CGDisplayRestoreColorSyncSettings`，过 1 秒再重新配置（`AppDelegate.displayReconfigured`，第 137–151 行）。
- **激活窗口**：维护一个几乎透明（alpha 0.01）、不接收鼠标的 1×1 窗口，每次改 gamma 时在 alpha 0.01 和 0.02 之间切一下（`createGammaActivityEnforcer` / `enforceGammaActivity`，第 17–42 行）。源码没写原因，推测是强制 WindowServer 重绘，让新 gamma 立即生效。**未验证**。
- **遮罩窗口**：另一种方案。在 `CGShieldingWindowLevel` 层级放一个黑色、不接收鼠标的全屏窗口，靠它的 alpha 调暗（第 65–83 行）。虚拟显示器或用户关掉 gamma 时使用。
- Apple 文档：`CGDisplayRestoreColorSyncSettings`「把 gamma 表恢复为用户 ColorSync 显示描述文件中的值」。进程退出后 gamma 是否自动复原，文档没写，**未验证**。

## 3. macOS 系统音量

- MonitorControl 不做系统音量兜底。它用默认输出设备决定音量键给谁：默认输出设备的名称和显示器名匹配，就经 DDC 调那台显示器；如果设备自己能调音量（`canSetVirtualMainVolume`），就把音量键交还系统（`MediaKeyTapManager.swift` 第 169–179 行，`DisplayManager.updateAudioControlTargetDisplays` 第 222–238 行）。
- 名称匹配规则：去掉括号、空格和数字后比较（`normalizedName`，第 212–220 行）；用户可以手填设备名覆盖（`audioDeviceNameOverride`）。
- **本机实测**：用 CoreAudio 遍历输出设备，查 `kAudioHardwareServiceDeviceProperty_VirtualMainVolume` 和 `kAudioDevicePropertyMute`（输出域）。

| 设备 | 传输类型 | 音量属性 | 静音属性 |
|---|---|---|---|
| HP D27k | `dprt`（DisplayPort） | 无 | 无 |
| HP D27k | `hdmi` | 无 | 无 |
| Mac mini 扬声器 | `bltn`（内建） | 有，可写 | 有，可写 |

结论：macOS 不给显示器音频设备提供音量控制，系统音量键对它是灰的。DDC 也不支持时，DeskBaton 在 Mac 上无法调这台显示器的音量。

## 4. Windows DDC

- Monitorian 两条路径都用：
  - 高层 API：`GetMonitorCapabilities` 报了 `MC_CAPS_BRIGHTNESS` 时，用 `GetMonitorBrightness` / `SetMonitorBrightness`。注释写着「`SetMonitorBrightness` 实际失败时也可能返回 true」（第 510 行）。
  - 低层 API：否则用 `GetVCPFeatureAndVCPFeatureReply` / `SetVCPFeature(0x10)`。
  - 对比度只走低层（第 460–463 行）。
- twinkle-tray `node-ddcci/ddcci.cc` 也是两条都用：`SetVCPFeature`（第 1277 行）和 `SetMonitorBrightness`（第 1416 行）。
- DeskBaton 现有 `windows.rs` 已经在用低层 API 读写 `0x60`。亮度、音量直接复用低层 API，不必引入高层 API。

## 5. Windows 软件调光

- **微软文档**（[SetDeviceGammaRamp](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-setdevicegammaramp)）：
  - 「强烈建议不要使用」。
  - 违反启发式检查（接近全黑或全白）的表会被静默拒绝：返回 TRUE，但不生效。
  - 其它应用随时可能覆盖，连接 / 断开显示器、改分辨率等多数显示事件也会重置。
  - HDR 下行为未定义；和色彩校准的交互也未定义。
  - 单次调用最长可能约 200 ms。
- **twinkle-tray 的做法**（`tt-windows-utils/windows_display_brightness.cc`、`electron.js` 第 1490–1650 行，`Monitors.js` 第 85–165 行）：
  - 只在设置里打开 `useSoftwareBrightnessFallback`、且显示器没有任何硬件亮度控制（`type === "none"`）时才启用，类型记为 `software`。
  - 亮度下限 20%（`kMinimumBrightness`）。低于 70% 时峰值固定在 70%（`kMinimumDriverScale`），只用幂曲线压中间调，避开 GDI 的静默拒绝。
  - 写入后回读，误差在 256 以内才算成功；失败重试 3 次，间隔 20 / 40 ms。
  - 每次写之前回读当前表；如果已经不是「基准表套上自己的曲线」，就认为被别人改了，以当前表为新基准。
  - 进程退出后 gamma 表还留着，所以启动时会从当前表反推出上次留下的亮度：只有三个通道的峰值都没到满幅，才认为是自己调暗的（f.lux、夜间模式至少有一个通道是满的）。
  - HDR 开着时不用 gamma（`canUseGammaRamp`）。
  - 显示事件后表恰好回到默认值，就视为被重置，重新应用（`restoreGammaAfterReset`）。
  - 退出时先停掉写入队列，再把调过的表复原（`restoreGammaRampsOnQuit`）。
- 同一个 `\\.\DISPLAYn` 下的多台物理显示器共用一张 gamma 表，twinkle-tray 按路径去重。

## 6. Windows 系统音量

- **微软文档**（[IAudioEndpointVolume](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nn-endpointvolume-iaudioendpointvolume)）：
  - 适配器有硬件音量 / 静音控制时直接用硬件；没有时由音频引擎做软件音量，只在共享模式下生效。
  - 用 `SetMasterVolumeLevelScalar`（0.0–1.0，听感曲线）和 `SetMute` 设置。
  - 用 `QueryHardwareSupport` 判断有没有硬件控制。
  - 文档提醒：改端点主音量会影响其它应用，通常应由用户在系统音量里控制。
- Monitorian 和 twinkle-tray 都不调系统音量。
- 显示器音频端点的名称通常形如「显示器名 (显卡厂商 High Definition Audio)」，可以按 EDID 型号做子串匹配。**未验证**，需在 Windows 机器上实测。

## 7. 依赖

现有依赖已经够用，不需要新增 crate：

- macOS：gamma 和 CoreAudio 放进 `baton-macos-sys/native`（ObjC），链接 CoreGraphics、CoreAudio。
- Windows：`windows` crate 补开 feature：`Win32_Graphics_Gdi`（gamma，已开）、`Win32_Media_Audio`、`Win32_Media_Audio_Endpoints`、`Win32_System_Com`、`Win32_UI_Shell_PropertiesSystem`（端点名称）。
- 不引入 `ddc` / `ddc-hi`：现有 `mccs.rs` 已覆盖所需报文。

## 来源

| 事实 | 来源 |
|---|---|
| VCP 编码、静音取值、最大值截断、音量至少写 1、写入合并 | MonitorControl `MonitorControl/Model/OtherDisplay.swift`、`Enums/Command.swift`、`main.swift` |
| macOS DDC 节奏 | MonitorControl `Support/Arm64DDC.swift` |
| gamma 软件调光、冲突检测、遮罩窗口 | MonitorControl `Model/Display.swift`、`Support/DisplayManager.swift` |
| 重配置 / 唤醒处理 | MonitorControl `Support/AppDelegate.swift` |
| 音量键归属与设备名匹配 | MonitorControl `Support/MediaKeyTapManager.swift`、`Support/DisplayManager.swift` |
| Windows 高低层 DDC、错误码 | Monitorian `Source/Monitorian.Core/Models/Monitor/MonitorConfiguration.cs` |
| Windows gamma 曲线、回读校验、复原 | twinkle-tray `src/modules/tt-windows-utils/windows_display_brightness.cc`、`src/electron.js`、`src/Monitors.js` |
| 写入节流 | twinkle-tray `src/electron.js` |
| `SetDeviceGammaRamp` 限制 | [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/nf-wingdi-setdevicegammaramp) |
| 端点音量与软件音量 | [Microsoft Learn](https://learn.microsoft.com/en-us/windows/win32/api/endpointvolume/nn-endpointvolume-iaudioendpointvolume) |
| `CGDisplayRestoreColorSyncSettings` 语义 | [Apple Developer](https://developer.apple.com/documentation/coregraphics/cgdisplayrestorecolorsyncsettings()) |
| HP D27k VCP、macOS 显示器音频设备无音量 | 本机实测（本文第 1、3 节） |
