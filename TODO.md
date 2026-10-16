# TODO

按优先级从上到下推进，规则见 `AGENTS.md`「推进」。标 🔧 的需要特定硬件或环境，由用户实测确认后才打勾。

## P0 · M0 原型：验证设计前提

- [x] Mac 端 DDC 读写
- [ ] 🔧 实测 P275MV：USB-C / DP 的输入源编码，KVM 是否跟随输入源。结果写进 `docs/research/`，不符合时先改设计文档
- [x] Mac `stay_awake`：IOKit 合盖睡眠控制 + 防空闲睡眠断言
- [x] Mac 启动时检查并清理残留的睡眠设置
- [ ] 🔧 验收清单第 1 项：合盖 Mac 切到 Windows，5 分钟内切回不用开盖
- [x] Windows 端 DDC 读写（Dxva2，型号和序列号取自注册表 EDID）+ `wake_display`
- [ ] 🔧 到 Windows 机器上跑 `deskbaton-cli displays / get-input / set-input` 验证
- [ ] 🔧 实测免配件唤醒：显示器「USB Power(Sleep)」打开后 KVM 键盘能否唤醒合盖 Mac；蓝牙 HID 能否唤醒
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
- [ ] mDNS 发现、指纹配对界面，配对信息独立存储
- [ ] 托盘、通知、开机自启
- [ ] Mac `stay_awake` 回退路径：IOKit 合盖控制失败时由特权助手执行 `pmset -a disablesleep 1`，并在启动时清理残留的 `SleepDisabled`
- [ ] 🔧 验收清单第 1、3、4、5 项

## 显示器面板（`docs/superpowers/specs/2026-10-16-display-panel-design.md`）

- [x] `display.inputs` 配置项 + `baton-display` 屏幕模式（macOS CoreGraphics、Windows EnumDisplaySettings）
- [x] `baton-gui`：Tauri 托盘面板，显示器信息 + 输入源切换，CI Linux 排除
- [x] 🔧 面板硬件验收清单第 1–3 项（Mac mini + 两台 HP D27k，2026-10-16 用户确认）
- [ ] 🔧 面板硬件验收清单第 4 项（Windows）
- [ ] 读取 capabilities 字符串自动识别输入口

## P3 · M3 扩展（另立设计文档）

- [ ] HID 唤醒配件（验收清单第 2 项）
- [ ] 剪贴板实时模式、文件快传
- [ ] 软件键鼠模式、不切屏模式、多机器、Linux

## 待定

- [ ] `DisplayInfo::select` 支持按完整 id / 序列号匹配，解决同型号多显示器无法区分（改核心匹配逻辑，需确认）
