# DeskBaton 零配置设计

日期：2026-10-17　状态：已实现，待硬件验收

主设计见 `2026-10-16-deskbaton-design.md`，配对见 `2026-10-17-pairing-design.md`，显示器面板见 `2026-10-16-display-panel-design.md`。本文修改这三份文档中的相应部分，冲突时以本文为准。

## 1. 目标

两台电脑各自打开 DeskBaton 面板，不写配置文件、不核对数字，就能配对并交棒：

- 局域网里发现对方后自动发起配对，每台电脑的面板上各点一次「同意」；
- 本机在显示器上对应哪个输入源，由程序自己学会；
- 面板按电脑而不是按接口列出交棒目标，点对端即交棒。

配置文件仍然可用，写了的字段优先于自动结果。

## 2. 范围

**做**

- 发现未配对设备后自动拨号，配对请求只需「同意 / 拒绝」，不显示配对码
- 本机输入源编码自动学习，存入数据目录；面板「⋯」中可重新识别
- 新增线上消息 `input_code`，`PROTOCOL_VERSION` 升到 5
- 面板「输入源」卡片改为「电脑」卡片，点对端触发交棒（同时完成 `TODO.md` 中「面板上点对端机器的输入口时改为触发交棒」）

**不做**

- 三台及以上电脑
- 不持棒的一方请求要回画面
- 读取显示器 capabilities 字符串识别输入口（仍在 `TODO.md`）
- 只需一端同意的配对：一端同意即双向信任时，同一网络里的任何人在自己机器上点同意就能被对方信任，将来会收到交棒与剪贴板

## 3. 配对：自动发起，各点一次同意

### 3.1 自动拨号

配对设计第 7 节「自动拨号的对象」增加一类：mDNS 发现的、未配对的设备，条件是

- 本进程有配对界面（面板）；无界面的 `deskbaton` 不自动拨未配对设备；
- 该指纹本次运行中没有被拒绝，也没有超时未处理；
- 只由名称较小的一方拨号，避免两边同时发起两个配对会话；
- mDNS 广播的协议版本与本机相同（版本不同时握手必然失败，自动拨只会反复报错）。

握手、承诺与信任判定（配对设计第 4、5 节）不变。双方都走到第 5 节第 4 行「发 `pair_pending`，弹出配对请求」，各自等本机用户同意。

### 3.2 面板

- 配对请求只显示对方名称、系统和 IP，以及「拒绝 / 同意」；不显示配对码。配对码仍在握手中计算，不再展示。
- 有新请求时照旧自动弹出面板。
- 自动发起的配对没完成（本机拒绝、对方拒绝、120 秒未处理）：本次运行不再自动拨号该设备；本机拒绝过的设备连进来也不再弹窗。「可配对」列表保留，对应按钮改名为「连接」，点击后重新发起一次（不受拒绝名单限制，与现在的「配对」相同）。

## 4. 本机输入源编码：自动学习

### 4.1 依据

显示器的 KVM 让键鼠跟着画面走：本机刚收到键鼠操作，说明画面此刻在本机，这时读到的输入源就是本机的编码。

合盖的 Mac 用 KVM 键鼠时成立。开盖用笔记本自带键盘、画面却在另一台时会学错；P275MV 上 Mac 读非活跃输入口只得到 Null Message，读取失败就不会学习。其它显示器靠 4.4 的重新识别纠正。

### 4.2 新增端口

```rust
/// 本机距离最近一次键鼠输入的时长。
pub trait InputActivity: Send + Sync {
    fn idle(&self) -> Result<Duration>;
}
```

| 平台 | 实现 |
|---|---|
| macOS | `CGEventSourceSecondsSinceLastEventType(kCGEventSourceStateHIDSystemState, kCGAnyInputEventType)`，FFI 声明放 `baton-macos-sys` |
| Windows | `GetLastInputInfo` 与 `GetTickCount` 之差 |
| 其它 | `Error::Unsupported` |

实现放 `baton-input` 的平台模块。

### 4.3 学习流程

`baton-core` 中的 `InputCodeLearner`（纯逻辑，用假实现单测），由 agent 在独立线程上每 2 秒调用一次：

1. 配置写了 `display.input_code`：不学习，始终用配置值。
2. 已学到编码：不再读取，避免持续占用 DDC。
3. `idle() < 2 秒` 且 `get_input()` 成功：记为本机编码。

学到后：

- 写入数据目录的 `state.toml`（程序维护，`deny_unknown_fields`，临时文件 + 重命名）：

  ```toml
  input_code = 0x10
  ```

- 发 `Event::LocalInputCode(code)` 给协调器，持棒判定与交棒改用它；
- 调用链路的 `set_local_input_code(code)`：之后的 `hello` 带上它，并向已连接的对端发 `input_code` 消息。

本机编码的来源优先级：配置 > `state.toml` > 未知。未知时持棒判定为 `Idle`，热键与面板提示「本机还没识别接口，请先在本机操作一次键鼠」。

### 4.4 重新识别

面板「⋯」菜单「重新识别本机接口」：删除 `state.toml` 中的编码，通知协调器与链路置为未知（对端收到 `input_code: null`），下一次满足 4.3 第 3 条时重新学习。配置写了 `display.input_code` 时该菜单项不可用。

### 4.5 协议 v5

```json
{"type":"input_code","input_code":16}
{"type":"input_code","input_code":null}
```

- 双向，任何时候都可发送；收到后更新内存中的 `PeerInfo` 与 `peers.toml` 的 `input_code`，并经 `Event::PeerUpdated(PeerInfo)` 通知协调器。
- `PROTOCOL_VERSION` 从 4 升到 5，格式稳定性测试增加此消息。

### 4.6 交棒时对端编码未知

持棒方选中的目标没有 `input_code`：不发 `prepare`，提示「XX 还没识别接口：先用显示器菜单切到 XX，在它上面操作一次键鼠」。

## 5. 面板：按电脑显示

### 5.1 「电脑」卡片

交棒显示器（`display.match` 命中的那台）上，「输入源」卡片改为「电脑」：

```text
┌──────────────────────────────────┐
│ 电脑                              │
│ ┌──────────────┐ ┌──────────────┐ │
│ │ [macOS 图标] │ │ [Win 图标]   │ │
│ │ LYMANYANG-MC0│ │ ptking-pc    │ │
│ │ 本机 · DP-1  │ │ 在线 · DP-2  │ │
│ └──────────────┘ └──────────────┘ │
└──────────────────────────────────┘
```

- 列出本机和已配对设备，本机在前，其余按名称排序。
- 每格：系统图标、设备名、小字一行（本机写「本机」，对端写「在线 / 离线」，后接接口名）。
- 接口名：配置 `display.inputs` 中有该编码时用配置的名称；否则按 MCCS 常见值命名（`0x01` VGA-1、`0x03` DVI-1、`0x04` DVI-2、`0x0F` DP-1、`0x10` DP-2、`0x11` HDMI-1、`0x12` HDMI-2、`0x1B` USB-C）；都不是时写编码。编码未知时写「未识别」。
- 当前画面所在的电脑高亮（显示器当前编码等于该电脑的编码）。本机读不到当前编码（不持棒）时，高亮心跳报告自己持棒的在线对端，「当前画面」行写它的接口名和设备名。

### 5.2 点击

- 点对端：发 `Event::HandoffTo(device_id)` 给协调器，走与热键相同的交棒流程（离线时先唤醒），只是目标由用户指定。结果经现有的 `notice` 显示在状态行。
- 点本机：不切换，提示「画面已在本机」或「请在当前显示画面的电脑上操作」。只有持棒方能发 DDC 命令。
- 交棒服务未启动时，退回原来的接口按钮（直接切输入源），状态行显示原因；与配对设计「agent 启动失败时面板照常可用」一致。

### 5.3 其它显示器

不是交棒显示器的外接屏（如 Mac mini 接两台 HP D27k 时的另一台）保持原来的接口按钮和直接切换，行为不变。

### 5.4 随交棒刷新

不持棒的一方读不到显示器，只能靠对端告知画面在谁那里；交棒由热键或对端发起时，面板也要跟着变。

- 链路记下每个连接最近一次心跳里的 `state`，经 `PairedStatus.holding` 交给面板。
- 本机状态变化时立即向已连接的对端补发一次心跳，不等下一个 2 秒周期；不改线上格式。
- 本机或对端的状态变为 `holding` / `idle` 时，链路调用 `LinkObserver::changed`，面板随之重读显示器；`handing_off` / `receiving` 是过渡状态，不触发刷新，避免与接棒方的 DDC 轮询抢总线。

## 6. 协调器

- `Event::HandoffTo(DeviceId)`：与 `Event::Hotkey` 共用同一段流程，区别只在目标选择。`Hotkey` 仍按配对设计第 8 节选目标。
- `Event::LocalInputCode(Option<InputCode>)`：更新本机编码，并重新判定持棒状态。
- `Event::PeerUpdated(PeerInfo)`：合并到已知设备，不改变在线状态。
- `my_input_code()` 改为读协调器内部的本机编码（初始化为配置值或 `state.toml`），不再直接读配置。

## 7. 代码落点

| 位置 | 内容 |
|---|---|
| `baton-core/src/ports.rs` | `InputActivity` |
| `baton-core/src/learn.rs` | `InputCodeLearner` |
| `baton-core/src/event.rs` | `HandoffTo`、`LocalInputCode`、`PeerUpdated` |
| `baton-core/src/protocol.rs` | v5：`input_code` 消息 |
| `baton-core/src/coordinator.rs` | 本机编码可变、指定目标交棒、对端编码未知的提示 |
| `baton-core/src/state_file.rs` | `state.toml` 读写 |
| `baton-input` | `InputActivity` 的 macOS / Windows 实现 |
| `baton-macos-sys` | `CGEventSourceSecondsSinceLastEventType` 的 FFI 声明 |
| `baton-net/src/link.rs` | 自动拨号未配对设备、`set_local_input_code`、收发 `input_code`、记下对端心跳状态 |
| `baton-agent/src/lib.rs` | 启动学习线程、加载 `state.toml` |
| `baton-gui` | 「电脑」卡片、`handoff_to` 与 `relearn_input` 命令、配对请求去掉配对码、「可配对」按钮改名 |

## 8. 测试

- **单测**：`InputCodeLearner` 四种情形（配置优先、已学到不读、空闲过久不学、读取失败不学）；`state.toml` 往返与未知字段报错；协议 v5 往返与格式稳定；协调器 `HandoffTo` 选中指定目标、对端编码未知时不发 `prepare`、`LocalInputCode` 后重新判定持棒；自动拨号对象的筛选（已拒绝、名称较大一方、无界面）；「电脑」卡片的排序、接口名、高亮。
- **回环**：两个 agent 都不配置 `input_code`，其中一端经 `set_local_input_code` 学到后，另一端的 `PeerInfo` 与档案随之更新；一端 `set_local_state(Holding)` 后，另一端的 `PairedStatus.holding` 随之变化。
- **硬件验收清单**（手动）：
  1. 两台电脑删除配置文件后打开面板，各点一次「同意」即配对，之后重启自动连上；
  2. 在每台电脑上操作一次键鼠后，两边「电脑」卡片都显示正确的接口名；
  3. 在 Mac 面板点 Windows、在 Windows 面板点 Mac，双向交棒成功，切回 Mac 不需要开盖；
  4. 「重新识别本机接口」后，操作一次键鼠能重新学到；
  5. 一端拒绝配对后，本次运行不再弹窗；在「可配对」点「连接」可以重新发起。

## 9. 风险

| 风险 | 应对 |
|---|---|
| 开盖使用笔记本自带键盘时学错编码 | 读取非活跃输入口失败时不学习；面板「重新识别本机接口」 |
| 首次使用时对端从未显示过、编码未知 | 交棒前提示用显示器菜单切过去操作一次 |
| 未配对设备被自动拨号，陌生设备反复弹窗 | 拒绝或超时后本次运行不再拨号 |
| 协议 v5 与旧版本不兼容 | 握手提示「协议版本不一致」，两端同时更新 |
