# TODO

按优先级从上到下推进，规则见 `AGENTS.md`「推进」。标 🔧 的需要特定硬件或环境，由用户实测确认后才打勾。

## P0 · M0 原型：验证设计前提

- [x] Mac 端 DDC 读写
- [ ] 🔧 实测 P275MV：USB-C / DP 的输入源编码，KVM 是否跟随输入源。结果写进 `docs/research/`，不符合时先改设计文档（[记录](docs/research/2026-10-16-p275mv-ddc-probe.md)：KVM 跟随已证实；USB-C 读到 `0x0F`，DP 口编码待在 Windows 上读，可能撞码）
- [x] Mac DDC 请求连发两次：P275MV 只收到一次请求时回 Null Message
- [ ] 🔧 用新 DDC 实现在 HP D27k 上复测 `get-input` / `set-input`
- [x] 合盖 Mac 切回时外接屏短暂消失触发合盖睡眠，IOKit `kPMSetClamshellSleepState` 会被 powerd 覆盖而失效：设计文档改为以 `pmset -a disablesleep 1` 为 `stay_awake` 主路径
- [x] 🔧 手动 `sudo pmset -a disablesleep 1` 后合盖切走再切回，Mac 不睡、切回后不开盖画面恢复（2026-10-16 用户确认）
- [x] Mac `stay_awake` 主路径：经 sudoers 规则执行 `sudo -n pmset -a disablesleep 1/0`，去掉 IOKit 合盖控制；规则缺失时只记警告；标记文件挪到 `~/Library/Application Support/DeskBaton/`；`deskbaton-cli sleep-helper install / uninstall / status`
- [x] 🔧 装好 sudoers 规则后用 `deskbaton-cli stay-awake` 复测合盖切走再切回：Mac 不睡，不开盖画面恢复（2026-10-16 用户确认）
- [x] 收到 SIGTERM / SIGHUP / Ctrl+C 时释放 `AwakeGuard`：agent 的 `ctrlc` 开 `termination` 特性；CLI `stay-awake` 等待信号或到期后再释放。否则 `SleepDisabled` 残留到下次启动
- [ ] 🔧 在 Windows 上停留超过 Mac 的 `displaysleep`（显示器已关、系统醒着）后切回：已实测失败两次（按键和 `caffeinate -u` 都没能恢复，推断 KVM 要等 USB-C 有视频信号才交出键鼠，见 P275MV 调研）。下一步用 `caffeinate -d` + `stay-awake` 验证「交出控制权期间防止显示器睡眠」，通过后再改设计文档和 `stay_awake`
- [x] Mac `stay_awake`：IOKit 合盖睡眠控制 + 防空闲睡眠断言
- [x] Mac 启动时检查并清理残留的睡眠设置
- [ ] 🔧 验收清单第 1 项：合盖 Mac 切到 Windows，5 分钟内切回不用开盖
- [x] Windows 端 DDC 读写（Dxva2，型号和序列号取自注册表 EDID）+ `wake_display`
- [x] CI 手动触发的 Windows 构建（`windows-build.yml`），产出 `deskbaton-cli.exe`、`deskbaton.exe`、`deskbaton-gui.exe` 供实测
- [ ] 🔧 到 Windows 机器上跑 `deskbaton-cli displays / get-input / set-input` 验证
- [x] 🔧 实测免配件唤醒：Windows 发 Wi-Fi 魔术包可唤醒合盖睡眠的 Mac，睡 45 秒、约 30 分钟均通过，不需要睡眠代理和 Bonjour 服务（2026-10-17 用户实测，[记录](docs/research/2026-10-17-mac-wifi-wol-probe.md)）；KVM 键盘、蓝牙路径不再必测
- [x] 把 HP D27k 实测记录到 `docs/research/2026-10-16-hp-d27k-ddc-probe.md`（切走后是否自动切回待用户确认）

## P1 · M1 命令行交棒

- [x] `baton-core`：补 `channel.rs`（`ChannelId`）、`participant.rs`（`HandoffParticipant`）、`event.rs`（`Event`），`PeerLink` 改为事件驱动
- [x] `Coordinator` 交棒流程（设计 6.2 第 1–10 步），用假实现单测正常交棒、各类超时、掉线、唤醒失败、并发热键
- [x] `baton-net`：帧格式 v2、`mux` 控制帧优先的发送调度、transfer 分块与重组（sha256 校验）
- [x] `baton-net`：TCP + TLS 连接、预共享令牌、2 秒心跳与离线判定
- [x] `peer_offline` 保持唤醒策略：对端离线时释放 `AwakeGuard`
- [x] `baton-agent`：组装各平台实现、全局热键、写死对端地址（`config.example.toml`）
- [x] 本机回环地址上两个 agent 跑通握手、心跳、双向交棒与随身数据（`crates/baton-agent/tests/loopback.rs`）
- [ ] 🔧 验收清单第 1、3、4 项

## P2 · M2 可用版本

- [ ] `baton-clipboard`：`ClipboardAccess`（arboard）+ `ClipboardSync`，防回传、超限跳过、敏感标记跳过
- 配对与设备档案（[设计](docs/superpowers/specs/2026-10-17-pairing-design.md)），按顺序推进：
  - [x] 持久化 TLS 身份、`peers.toml` 设备档案、握手 v3（承诺式配对码、信任判定、令牌自动配对），`PROTOCOL_VERSION` 升到 3
  - [x] 档案驱动唤醒：启动时把档案设备交给协调器，`wol_mac` 档案优先，`WakeTarget.last_ip`；`hello.wol_mac` 上报承载连接的网卡当前 MAC；`WolWaker` 同时发往 `255.255.255.255`、子网广播和最后已知 IP
  - [x] mDNS 广播与浏览，拨号地址依次取 mDNS、档案、配置
  - [x] `baton-agent` 抽成库 `Agent::start`，面板进程内运行 agent 与热键，保留无界面 `deskbaton`
  - [x] 面板「设备」卡片：配对请求（6 位码、同意 / 拒绝）、已配对设备、可配对设备与发起配对、取消配对
  - [ ] 🔧 配对设计验收清单第 1–5 项
- [ ] 托盘通知、开机自启
- [ ] 面板上点对端机器的输入口时改为触发交棒，而不是直接切输入源
- [ ] Mac 首次运行检查 `womp` 已开、当前网络的私有 Wi-Fi 地址为「固定」，不满足时提示
- [ ] 🔧 测魔术包触发的暗唤醒在无人干预时能持续多久；不够 agent 重连并回心跳时，`kern.wakereason` 含 `wifibt` 就短暂保持唤醒、等 `prepare`
- [ ] 🔧 Wi-Fi 魔术包唤醒：睡眠超过 1 小时（验收清单第 2 项）、电池供电
- [ ] 🔧 验收清单第 1–5 项

## 显示器面板（`docs/superpowers/specs/2026-10-16-display-panel-design.md`）

- [x] `display.inputs` 配置项 + `baton-display` 屏幕模式（macOS CoreGraphics、Windows EnumDisplaySettings）
- [x] `baton-gui`：Tauri 托盘面板，显示器信息 + 输入源切换，CI Linux 排除
- [x] 🔧 面板硬件验收清单第 1–3 项（Mac mini + 两台 HP D27k，2026-10-16 用户确认）
- [ ] 🔧 面板硬件验收清单第 4 项（Windows）
- [x] 输入源按钮标出占用的设备（本机 / 已配对设备的名称与系统）；`hello` 增 `platform`，`PROTOCOL_VERSION` 升到 4
- [ ] 🔧 P275MV + 本机配置（USB-C `0x0F`）下面板显示「USB-C · 本机」；与 Windows 配对后，其输入口标出设备名和 Windows
- [ ] 设备上线、下线或档案变化时刷新输入源按钮上的设备（目前只在打开面板时刷新）
- [ ] 读取 capabilities 字符串自动识别输入口

## P3 · M3 扩展（另立设计文档）

- [ ] HID 唤醒配件（Wi-Fi 魔术包不可用时的备选）
- [ ] 剪贴板实时模式、文件快传
- [ ] 软件键鼠模式、不切屏模式、多机器、Linux

## 待定

- [ ] `DisplayInfo::select` 支持按完整 id / 序列号匹配，解决同型号多显示器无法区分（改核心匹配逻辑，需确认）
