# 离线切换与 Windows 保持唤醒

日期：2026-10-18　状态：已确认

主设计见 `2026-10-16-deskbaton-design.md`。冲突时以本文为准。

## 1. 目标

人在 Mac 上时，Windows 关机、睡眠、或已按电源但还没进桌面（DeskBaton 未启动），点 Windows 仍要把画面和键鼠切过去。不为此做开机服务。

合盖睡眠的 Mac 被切换时，仍先唤醒再切：暗唤醒没有视频信号，必须等 agent 上线并 `wake_display`。

## 2. 范围

**做**

- 档案里 `platform` 为 Windows / Linux / 其它时：对端离线也直接 `set_input`，同时尽最大努力走唤醒链
- 这样切过去之后不等 `taken`，超时不切回
- Windows 的 `stay_awake` 真正阻止系统睡眠（对端在线期间）

**不做**

- Windows 登录前服务 / Session 0 agent
- 改线上协议或 `PROTOCOL_VERSION`
- 离线时同步剪贴板（实时通道本来就发不出去）
- 为「已切到离线对端」单独做面板高亮；状态行通知即可

## 3. 离线切换

持棒方已有对端输入源编码（档案或上次握手）时才能切。没有编码则仍提示先用显示器菜单切过去、在那台机器上操作一次键鼠。

### 3.1 非 Mac（Windows 等）

对端离线时：

1. 运行唤醒链（成功、需要用户、失败都不阻断切换）。
2. `input.release`，`set_input(对端编码)`。不发 `prepare`，不 `stay_awake`（对端已经不在线，`peer_offline` 的长时 guard 没有释放点）。
3. 本机进入 `Idle`。通知「已切到 {名称}（对方不在线）」；若已发唤醒信号或无法唤醒，把该句接在后面。
4. 不进入 `AwaitTaken`。之后即使 8 秒内没有 `taken`，也不把输入源切回来。

对端之后上线：启动时 `refresh_state`，`get_input` 等于本机编码即进入 `Holding`。回切仍要那台机器上的 DeskBaton 已运行；更早只能用显示器 OSD。

在线对端仍走原来的 `prepare` / `ready` / `taken`。

### 3.2 Mac

`platform` 为 `macos`，或档案里还没有 `platform`（按合盖 Mac 处理，避免暗唤醒后无视频就切走）：

保持主设计 6.2 第 2–4 步：唤醒链 → 等心跳 → `prepare` → 接棒方 `stay_awake` + `wake_display` → 再 `set_input`。唤醒等待超时仍只通知，不切画面。

### 3.3 `platform` 的含义

`hello.platform` 除了给面板画系统名，还决定离线切换策略。设备档案必须继续记下它。

## 4. Windows `stay_awake`

交出控制权后，Windows 按与 Mac 相同的 `stay_awake_until` 策略保持唤醒，避免人在 Mac 上用的时候 Windows 空闲睡死。

- 用 `PowerCreateRequest` + `PowerSetRequest(PowerRequestSystemRequired)`，不要 `PowerRequestDisplayRequired`（共用屏已经在显示另一台）。
- 多个 guard 走现有 `SharedSwitch`；最后一个释放时 `PowerClearRequest` 并关掉句柄。
- 这是进程内请求，进程退出即失效，`restore_sleep` 为空操作，不留标记文件。
- `wake_display` 仍用一次 `SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)` 点亮输出，不带 `ES_CONTINUOUS`。

接棒前那 60 秒的 `stay_awake` 在 Windows 上不再是空操作，同样拦住系统睡眠。

## 5. 主设计对应修改

- 6.2 第 2 步按 §3 分平台。
- 6.2 第 4 步去掉「Windows 上为空操作」。
- 6.2 第 9 步：离线直接切的那次不适用「超时切回」。
- 5.3：Windows `stay_awake` 按 §4。
- 错误表：「唤醒链全部失败」在非 Mac 离线切换时仍切画面，只把原因附在通知里。

## 6. 测试与验收

**单测（假显示器、假唤醒）**

- 离线 Windows（档案有编码）：热键后立刻切到 Windows 编码，状态 `Idle`，不发 `prepare`，8 秒后画面仍在 Windows
- 同上，唤醒失败或 `NeedsUser`：仍然切
- 离线 Windows 没有编码：不切，提示识别接口
- 离线 Mac（或没有 `platform`）：仍先唤醒再等上线，不立刻切
- 在线交棒路径不变

**硬件**

- 🔧 Windows 关机后按电源、未进桌面，Mac 点 Windows，立刻看到开机或登录画面
- 🔧 人在 Mac 上超过 Windows 空闲睡眠时间，Windows 不睡，能切回去
- 合盖睡眠 Mac 从 Windows 切过去，仍走唤醒 + `prepare`（主设计验收清单第 2 项，不改）
