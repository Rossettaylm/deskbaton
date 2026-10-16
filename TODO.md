# TODO

按优先级从上到下推进，规则见 `AGENTS.md`「推进」。标 🔧 的需要特定硬件或环境，由用户实测确认后才打勾。

## P0 · M0 原型：验证设计前提

- [x] Mac 端 DDC 读写
- [ ] 🔧 实测 P275MV：USB-C / DP 的输入源编码，KVM 是否跟随输入源。结果写进 `docs/research/`，不符合时先改设计文档
- [ ] Mac `stay_awake`：IOKit 合盖睡眠控制 + 防空闲睡眠断言；失败时回退到特权助手执行 `pmset -a disablesleep 1`
- [ ] Mac 启动时检查并清理残留的 `SleepDisabled`
- [ ] 🔧 验收清单第 1 项：合盖 Mac 切到 Windows，5 分钟内切回不用开盖
- [ ] Windows 端 DDC 读写（Dxva2），🔧 到 Windows 机器上跑 `deskbaton-cli` 验证
- [ ] 🔧 实测免配件唤醒：显示器「USB Power(Sleep)」打开后 KVM 键盘能否唤醒合盖 Mac；蓝牙 HID 能否唤醒
- [ ] 把 HP D27k 实测（HDMI 读回 `0x11`、m1ddc 读回 `0`、切走后能否自动切回）记录到 `docs/research/`

## P1 · M1 命令行交棒

- [ ] `baton-core`：补 `channel.rs`（`ChannelId`）、`participant.rs`（`HandoffParticipant`）
- [ ] `Coordinator` 交棒流程（设计 6.2 第 1–10 步），用假实现单测正常交棒、各类超时、掉线、唤醒失败、并发热键
- [ ] `baton-net`：`mux` 控制帧优先的发送调度，单测「大数据传输期间控制帧最多等一个块」
- [ ] `baton-net`：TCP + TLS 连接、预共享令牌、2 秒心跳与离线判定
- [ ] `peer_offline` 保持唤醒策略：对端心跳消失时释放 `AwakeGuard`
- [ ] `baton-agent`：组装各平台实现、全局热键、写死对端地址
- [ ] 本机回环地址上两个 agent 跑通握手、心跳、交棒消息
- [ ] 🔧 验收清单第 1、3、4 项

## P2 · M2 可用版本

- [ ] `baton-clipboard`：`ClipboardAccess`（arboard）+ `ClipboardSync`，防回传、超限跳过、敏感标记跳过
- [ ] 剪贴板通道 transfer（Begin / Chunk / End / Cancel + sha256 校验）
- [ ] mDNS 发现、指纹配对界面，配对信息独立存储
- [ ] 托盘、通知、开机自启
- [ ] 🔧 验收清单第 1、3、4、5 项

## P3 · M3 扩展（另立设计文档）

- [ ] HID 唤醒配件（验收清单第 2 项）
- [ ] 剪贴板实时模式、文件快传
- [ ] 软件键鼠模式、不切屏模式、多机器、Linux

## 待定

- [ ] `DisplayInfo::select` 支持按完整 id / 序列号匹配，解决同型号多显示器无法区分（改核心匹配逻辑，需确认）
