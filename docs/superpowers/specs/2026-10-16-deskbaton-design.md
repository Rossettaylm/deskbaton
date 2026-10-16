# DeskBaton 首版设计

> 状态：架构已确认（2026-10-16），工程骨架已落地。背景调研见 `docs/research/2026-10-16-kvm-input-switch-landscape.md`。
>
> 修订（2026-10-16）：
> - Mac 不再长期阻止睡眠，改为"交棒后短时保持唤醒 + 唤醒链接管"；
> - 为纯软件（非 KVM）模式和多台机器预留扩展点。

## 1. 目标

一台显示器、两台电脑（Windows 台式机 + 合盖使用的 Apple Silicon MacBook Pro），按一个热键，把画面和键鼠一起交给另一台电脑。
Mac 平时可以正常睡眠，**被交棒时能从睡眠中唤醒并接管键鼠，不需要开盖**。

## 2. 范围

**首版做**

- 两台机器：Windows ×1 + Apple Silicon Mac ×1
- 单台显示器，通过 DDC/CI 写 VCP `0x60` 切输入源
- 键鼠由显示器内置 KVM 跟随输入源（`InputRouting::MonitorKvm`）
- Mac 交棒后短时保持唤醒（默认 300 秒，可设为 0），到期后允许正常睡眠
- 接棒方睡眠时，按配置依次尝试唤醒方式：WoL → HID 唤醒配件 → 提示用户
- 局域网配对 + 加密通信

**预留扩展点，首版不实现**

- 纯软件键鼠模式（`InputRouting::Software`，后端为 Deskflow 进程或自研）
- 不切屏模式（`DisplayMode::None`，每台电脑各有显示器）
- 三台及以上机器（配置和协议里的对端都是列表）

**不做**

- 剪贴板、文件传输；Linux、Intel Mac；跨网段 / 公网

## 3. 硬件与环境假设

| 项目 | 值 | 是否已验证 |
|---|---|---|
| 显示器 | 泰坦军团 P275MV（Mini-LED，带 KVM，USB-C 65W PD） | 型号已确认 |
| Windows 接法 | DP 视频 + USB-B 上行，有线网 | 已确认 |
| Mac 接法 | 单根 USB-C（视频 + USB + 供电），Wi-Fi，合盖 | 已确认 |
| KVM 跟随输入源（DP ↔ USB-B，USB-C ↔ USB-C） | 来自 MAX 版的社区实测 | **未验证** |
| 输入源编码：DP = `0x0F`，USB-C = `0x1B` | 业界常见值 | **未验证**，做成配置项 |
| 显示器只接受当前活跃输入口发来的 DDC 命令 | 按最坏情况设计 | 不需要验证 |
| 合盖睡眠的 Mac 无法被 KVM 切过来的键盘唤醒 | 用户实际经历 + 推断（键盘在睡眠后才接入，未登记为唤醒源） | 现象已确认，原因未验证 |

## 4. 架构总览

两台机器各运行一个 `deskbaton` agent，结构相同。核心层只依赖端口接口（trait），各平台实现由 agent 组装后注入。

```mermaid
flowchart LR
    subgraph Core["baton-core（与平台无关）"]
        CO[交棒协调 Coordinator]
        WC[唤醒链 WakeChain]
    end
    subgraph Ports["端口实现（按平台）"]
        DC[DisplayControl<br/>baton-display]
        PW[PowerControl / Waker<br/>baton-power]
        IR[InputRouter<br/>baton-input]
        PL[PeerLink<br/>baton-net]
    end
    HK[热键] --> CO
    CO --> DC & PW & IR & PL
    CO --> WC --> PW
    DC -- DDC/CI --> MON[显示器]
    PL <-- TLS / 局域网 --> PEER[对端 agent]
```

**关键不变量**：任一时刻，键鼠和画面都在同一台机器上，这台机器就是"持棒方"。只有持棒方会响应热键并发 DDC 命令。

## 5. 模块

### 5.1 代码组织（Cargo workspace）

```text
crates/
  baton-core/       领域类型、协议消息、配置、端口接口、唤醒链、协调器。不依赖系统 API，可完整单测
    src/ids.rs          DeviceId / HandoffId / DisplayId / InputCode / MacAddr
    src/config.rs       Config、DisplayMode、InputRouting、PowerConfig、WakeMethod
    src/protocol.rs     Message（JSON，tag = type）
    src/state.rs        AgentState
    src/ports.rs        DisplayControl / PowerControl / Waker / InputRouter / PeerLink
    src/wake.rs         WakeChain：按配置顺序尝试唤醒方式
    src/coordinator.rs  Coordinator + Ports
  baton-display/    DisplayControl：macos（经 baton-macos-sys）、windows（Dxva2）；quirks 厂商怪癖
  baton-power/      PowerControl：macos、windows；wake/ 下为 WolWaker、HidDongleWaker、PromptWaker
  baton-input/      InputRouter：MonitorKvmRouter；software/ 下为 SoftwareRouter + InputShareBackend（deskflow、native）
  baton-net/        discovery（mDNS 常量）、frame（长度前缀分帧）、pairing、TcpPeerLink
  baton-macos-sys/  ObjC 胶水（native/*.m）+ FFI 声明，只在 macOS 编译
  baton-agent/      常驻程序 deskbaton
  baton-cli/        调试工具 deskbaton-cli：displays / get-input / set-input / wol / stay-awake
```

**扩展方式**：新增一种能力只需新增对应 trait 的实现并在 agent 里注入，不改核心层。例如：

- 纯软件模式：实现 `InputShareBackend`；
- 新的唤醒方式：实现 `Waker` 并在 `WakeMethod` 加一个枚举值；
- Linux：给 `DisplayControl` / `PowerControl` 加 `linux` 模块。

### 5.2 DisplayControl

```rust
pub trait DisplayControl: Send + Sync {
    fn list(&self) -> Result<Vec<DisplayInfo>>;
    fn get_input(&self, display: &DisplayId) -> Result<InputCode>;   // 多数显示器只在本机是活跃输入时响应
    fn set_input(&self, display: &DisplayId, input: InputCode) -> Result<()>;
}
```

| 平台 | 实现 |
|---|---|
| Windows | `GetPhysicalMonitorsFromHMONITOR` + `GetVCPFeatureAndVCPFeatureReply` / `SetVCPFeature`（Dxva2） |
| macOS | 移植 m1ddc（MIT）的 IOAVService 读写到 `baton-macos-sys/native/ddc.m` |

目标显示器由 `DisplayInfo::select` 选择：配置为 `auto` 时要求恰好一台，否则按型号子串匹配。

### 5.3 PowerControl 与唤醒链

```rust
pub trait PowerControl: Send + Sync {
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard>;  // 含合盖睡眠；到期或 guard 释放后恢复
    fn wake_display(&self) -> Result<()>;
}
pub trait Waker: Send + Sync {
    fn method(&self) -> WakeMethod;
    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome>;      // Sent / NeedsUser(提示)
}
```

**Mac 的 `stay_awake`**：先试 IOKit 合盖睡眠状态控制（据称不需要 root，M0 验证），失败再回退为特权助手执行 `pmset -a disablesleep 1`；同时持有防空闲睡眠断言。只在两种场景短时使用：

1. Mac 交出控制权后的 `power.stay_awake_secs` 秒内（默认 300，覆盖"切过去马上又切回来"）；
2. Mac 被唤醒后、交棒完成前（约 60 秒），防止它在显示器还没切过来时又因合盖睡回去。

**唤醒链**（`WakeChain`，顺序由 `power.wake_chain` 配置）：

| 方式 | 适用对象 | 首版状态 |
|---|---|---|
| `Wol` | 有线网络的 Windows | 已实现 |
| `HidDongle` | 合盖睡眠的 Mac：常插在 Mac 上的 USB / BLE HID 配件，收到局域网指令后模拟一次按键。因为它在 Mac 睡眠前就已接入，可以作为唤醒源 | 接口已留，**提前到 M1 做原型** |
| `Prompt` | 兜底 | 已实现 |

M0 还要实测两条不需要配件的路径，任一条可行就作为 `Waker` 加入：

- 显示器"USB Switch"菜单 / "USB Power(Sleep)"打开后，KVM 切过来的键盘能否唤醒；
- 蓝牙 HID（Mac 设置"允许蓝牙设备唤醒"）由 Windows 侧发出能否唤醒。

### 5.4 InputRouter

```rust
pub trait InputRouter: Send + Sync {
    fn acquire(&self) -> Result<()>;                 // 本机成为持棒方后
    fn release(&self, to: &DeviceId) -> Result<()>;  // 本机交出控制权前
}
```

- `MonitorKvmRouter`：空实现，键鼠由显示器 KVM 跟随。
- `SoftwareRouter`：委托给 `InputShareBackend::focus(target)`。后端有两种：Deskflow（独立进程，保持与 GPL-2.0 代码的进程隔离）和自研（参考 Lan Mouse）。

### 5.5 PeerLink

- **发现**：mDNS 服务类型 `_deskbaton._tcp.local.`，默认端口 47800；配置里的 `peers[].address` 作为兜底。
- **配对**：首次连接时双方显示同一个由证书指纹派生的 6 位数字，用户确认后固定对方指纹。
- **传输**：TCP + TLS（rustls，自签证书 + 指纹固定）；帧格式为 4 字节大端长度 + JSON，单帧不超过 1 MiB。
- **心跳**：每 2 秒一次，连续 3 次没收到就判定对端离线。

### 5.6 协议消息

| 消息 | 方向 | 含义 |
|---|---|---|
| `hello { device_id, protocol_version, input_code, wol_mac }` | 双向 | 交换身份、本机输入源编码、WoL MAC |
| `heartbeat { state }` | 双向 | 状态同步 |
| `prepare { handoff_id }` | 持棒方 → 接棒方 | 准备接棒 |
| `ready { handoff_id }` | 接棒方 → 持棒方 | 已点亮显示输出 |
| `taken { handoff_id }` | 接棒方 → 持棒方 | 已确认成为活跃输入 |
| `abort { handoff_id, reason }` | 双向 | 放弃本次交棒 |

## 6. 交棒流程

### 6.1 状态

`Holding`（持棒）/ `Idle`（不持棒）/ `HandingOff(id)` / `Receiving(id)`。
启动时 `refresh_state`：`get_input` 读到本机的输入源编码即为 `Holding`，否则为 `Idle`。

### 6.2 流程（持棒方 A 按下热键）

1. A 检查 B 的心跳。
2. B 离线时，A 运行唤醒链。结果为 `Sent` 就等 B 的心跳（最多 30 秒）；结果为 `NeedsUser` 就弹通知并结束。
3. B 上线（或被唤醒）后，如果 B 是 Mac，立即 `stay_awake(60s)`，防止合盖睡回去。
4. A → `prepare`。B 调用 `wake_display()`，回 `ready`（3 秒超时）。
5. A 调用 `input.release(B)`，如果 A 是 Mac，再调用 `stay_awake(power.stay_awake_secs)`。
6. A 调用 `set_input(B 的输入源编码)`；KVM 模式下，键鼠随之切给 B。
7. B 每 300 毫秒轮询一次 `get_input`，读到自己的编码就调用 `input.acquire()`，回 `taken`，进入 `Holding`，并释放第 3 步拿到的 guard。最长等 8 秒。
8. A 收到 `taken` 后进入 `Idle`。A 是 Mac 时，第 5 步的 guard 到期后，Mac 正常睡眠。
9. A 超时没收到 `taken`：尝试 `set_input(自己的编码)` 切回，并通知报错。

## 7. 错误处理

| 场景 | 处理 |
|---|---|
| DDC 写失败 | 重试 2 次，间隔 200 毫秒；仍失败就中止并通知 |
| `stay_awake` 两种机制都失败 | 记录警告，继续交棒（只影响"短时内切回"的体验）；托盘提示授权特权助手 |
| 唤醒链全部失败 | 通知用户手动唤醒对端 |
| 收到 `taken` 前对端掉线 | 按超时处理，尝试切回 |
| agent 崩溃或退出 | `AwakeGuard` drop 时恢复；下次启动时检查并清理残留的 `SleepDisabled` |
| 显示器"自动选择输入源"把画面跳回去 | 第 7 步超时后报错，提示关闭该选项 |

## 8. 配置

路径：macOS 为 `~/Library/Application Support/DeskBaton/config.toml`，Windows 为 `%APPDATA%\DeskBaton\config.toml`。所有字段都有默认值。

```toml
hotkey = "Ctrl+Alt+Space"
handoff_timeout_ms = 8000

[display]
mode = "ddc"            # ddc | none
match = "auto"          # 或 EDID 型号子串
input_code = 0x1B       # 本机在显示器上对应的输入源编码

[input]
mode = "monitor_kvm"    # monitor_kvm | software（需再写 backend = "deskflow" | "native"）

[power]
stay_awake_secs = 300
wake_chain = ["wol", "hid_dongle", "prompt"]

[[peers]]
name = "windows-desktop"
address = "192.168.1.20:47800"
wol_mac = "aa:bb:cc:dd:ee:ff"
```

配对信息（对端指纹）单独存放，不写在配置文件里。

## 9. 测试策略

- **baton-core**：用假实现驱动协调器和唤醒链，覆盖正常交棒、各类超时、掉线、唤醒失败、并发热键。
- **baton-net**：分帧编解码单测；两个 agent 在本机回环地址上跑通配对、心跳、交棒消息。
- **CI**：macOS / Windows / Linux 三平台跑 `fmt`、`clippy -D warnings`、`test`。
- **硬件验收清单**（手动）：
  1. 合盖 Mac 切到 Windows 后，5 分钟内切回，不需要开盖；
  2. 合盖 Mac 切走并睡眠超过 1 小时后，从 Windows 交棒，Mac 被唤醒并接管键鼠，不需要开盖；
  3. Windows 睡眠时从 Mac 切过去，WoL 生效；
  4. 关闭 Mac 上的 agent 后，系统睡眠策略恢复正常。

## 10. 里程碑

| 阶段 | 内容 | 退出条件 |
|---|---|---|
| M0 原型 | ① Mac 端 DDC 读写；② Windows 端 DDC 读写；③ 实测 P275MV 的输入源编码和 KVM 跟随；④ Mac `stay_awake` 两种机制；⑤ 实测不需要配件的唤醒路径（KVM 键盘、蓝牙） | `deskbaton-cli` 在两端完成切换；验收清单第 1 项通过；明确合盖睡眠 Mac 的可行唤醒方式 |
| M1 命令行交棒 + 唤醒配件原型 | 无界面 agent，写死对端地址，用预共享令牌代替配对；ESP32-S3 / RP2040 HID 配件原型（若 M0 第 ⑤ 项不可行） | 热键完成双向交棒；验收清单第 2 项通过 |
| M2 可用版本 | 托盘、mDNS 发现、配对界面、开机自启、通知 | 验收清单全部通过 |
| M3 扩展 | 软件键鼠模式、不切屏模式、多机器、Linux | 另立设计文档 |

## 11. 风险

| 风险 | 影响 | 应对 |
|---|---|---|
| 合盖睡眠的 Mac 找不到不需要配件的唤醒方式 | 长时间睡眠后接管必须依赖配件 | 配件作为正式方案推进；短时保持唤醒覆盖高频的来回切换 |
| 无需 root 的合盖控制在新版 macOS 上失效 | 需要特权助手，安装体验变差 | 两条路径都实现，运行时探测 |
| P275MV 的 KVM 实际不跟随输入源 | KVM 模式不成立 | 切换到 `InputRouting::Software`，把软件键鼠模式提前 |
| macOS 应用公证需要 Apple 开发者账号 | 未公证的应用首次打开会被拦 | 先在文档里写明处理方式，后续再申请账号 |
| GPL 许可证 | 对外分发必须开源 | 已确定以 GPL 开源 |
