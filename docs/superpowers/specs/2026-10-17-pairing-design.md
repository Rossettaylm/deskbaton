# DeskBaton 配对、设备档案与面板配对设计

日期：2026-10-17　状态：已确认，实现中

主设计见 `2026-10-16-deskbaton-design.md`，显示器面板见 `2026-10-16-display-panel-design.md`。本文替换主设计中「预共享令牌 + 写死对端地址」的 M1 做法。

## 1. 目标

开着 DeskBaton 的设备在局域网里互相发现，在面板上核对一次 6 位数字完成配对；之后每次启动自动建立长连接，交棒、剪贴板等能力都跑在这条连接上。对端睡着、连接断开时，用在线期间记下的设备档案（MAC、IP）发魔术包唤醒它。

## 2. 范围

**做**

- 本机 TLS 身份持久化，证书指纹作为设备身份
- 配对：双方面板显示同一个 6 位配对码，双方都同意后互相固定指纹
- 设备档案 `peers.toml`：指纹、名称、输入源编码、WoL MAC、最后已知 IP 与端口，每次握手刷新
- mDNS 发现：广播本机、列出局域网里的其他设备
- agent 并入面板进程（`deskbaton-gui`），保留无界面的 `deskbaton` 供调试
- 面板「设备」卡片：配对请求、已配对设备、可配对设备

**不做**

- 三台及以上机器的热键选目标（档案与协议已是列表，选目标留到 M3）
- 系统通知、开机自启（仍在 `TODO.md` 的 M2 托盘项）
- 面板切输入源改为触发交棒（另列 `TODO.md`）

## 3. 本机身份

- 首次启动生成自签证书（rcgen，ECDSA P-256，有效期到 4096 年，靠指纹固定而不靠过期），存于数据目录 `identity/cert.der` 与 `identity/key.der`（PKCS#8）。
- 数据目录即配置文件所在目录：macOS `~/Library/Application Support/DeskBaton/`，Windows `%APPDATA%\DeskBaton\`。
- 私钥文件在 Unix 上以 `0600` 创建；Windows 依赖 `%APPDATA%` 的默认 ACL（仅当前用户）。
- 指纹 = 证书 DER 的 SHA-256。设备身份以指纹为准；`DeviceId` 仍是设备名称，供协调器和日志使用。
- 读取失败（文件损坏）时报错，不静默重新生成：重新生成会让所有已配对设备失效。

## 4. 握手（协议 v3）

```text
拨号方 C                                 接受方 S
—— TLS 1.3 双向证书：只验证对端持有证书私钥 ——
C → S  hello
C → S  pair_commit { commit = SHA256(n_c) }
                                         S 收到 C 的 hello 与 commit 后：
S → C  hello
S → C  pair_nonce { nonce = n_s }
C → S  pair_reveal { nonce = n_c }      S 校验 SHA256(n_c) == commit，不符即断开
双方各自判定信任（第 5 节），并各发一条：
  pair_accept                已信任对方
  pair_pending               需要本机用户确认，请对方也显示配对码；之后再发 pair_accept 或断开
双方都「已发出 pair_accept 且收到对方的 pair_accept」后登记连接、报告上线、开始心跳
```

- `hello { device_id, protocol_version, input_code, wol_mac, listen_port, auth }`：
  - `wol_mac`：承载本连接的网卡当前的 MAC（按 TCP 连接的本地 IP 查网卡）。Mac 走 Wi-Fi 时即 `en0` 当前在用的私有地址；
  - `listen_port`：本机监听端口，对端据此记下可回拨的地址；
  - `auth`：配置了 `network.token` 时为令牌对「角色、发送方名称、双方证书指纹」的 HMAC，否则省略。
- `n_c`、`n_s` 为 32 字节随机数。**配对码** = `SHA256("deskbaton-pair-v1" ‖ fp_C ‖ fp_S ‖ n_c ‖ n_s)` 前 4 字节（大端 u32）对 1 000 000 取模，补零到 6 位。
- 承诺先行使中间人无法挑选随机数凑出两边相同的配对码：拨号方先承诺 `n_c`，接受方在看到承诺后才给出 `n_s`，每次尝试成功率为百万分之一。
- 每次握手都走完整流程（包括已配对设备），不按条件分支。
- 等待对方 `pair_accept` 的超时：默认 5 秒；收到 `pair_pending` 后延长到 120 秒（等用户操作）。
- v2 中「接受方等到拨号方登记后的第一帧心跳才登记」的规则由 `pair_accept` 取代：拨号方因名称或指纹不符而拒绝时不会发 `pair_accept`，接受方不会误报上线。
- 线上格式变化：`PROTOCOL_VERSION` 从 2 升到 3；版本不一致直接断开并提示两端同时升级。

## 5. 信任判定

收到对端 `hello` 并完成 `pair_reveal` 后，按顺序判定：

| 顺序 | 条件 | 结果 |
|---|---|---|
| 1 | 拨号时指定了期望的指纹或名称，而对端不符 | 断开（地址上换了设备） |
| 2 | 对端指纹在档案里 | 信任；名称变了就更新档案 |
| 3 | 两端都配置了 `network.token` 且 `auth` 校验通过 | 信任，自动写入档案 |
| 4 | 本进程有配对界面，且该指纹本次运行中没被用户拒绝过（用户主动发起配对时不受此限） | 发 `pair_pending`，弹出配对请求 |
| 5 | 其它 | 断开；无界面时日志提示「在面板中配对，或两端配置相同的 network.token」 |

- 用户同意：写入档案（同名旧条目被替换，用于对端重装后重新配对），发 `pair_accept`。
- 用户拒绝或 120 秒未操作：断开，该指纹记入本次运行的拒绝名单，之后它再连进来直接断开、不再弹窗。
- 本机已信任、对端发来 `pair_pending`：面板仍显示配对码供对方核对，本机无需再点同意。
- 同一对端同时有两个配对会话（双方同时点了配对）：保留由名称较小一方拨出的会话，与连接去重规则一致。

## 6. 设备档案

文件：数据目录下的 `peers.toml`，由程序维护，`deny_unknown_fields`，新增字段须有默认值。

```toml
[[peers]]
name = "windows-desktop"
fingerprint = "9f86d081…"   # 64 位十六进制
paired_at = 1792300000      # Unix 秒
input_code = 0x0F
wol_mac = "aa:bb:cc:dd:ee:ff"
last_ip = "192.168.1.20"
port = 47800
```

- 写入用「临时文件 + 重命名」，避免写一半时崩溃损坏档案。
- 每次握手成功都刷新 `input_code`、`wol_mac`、`last_ip`、`port`、`name`；`hello.wol_mac` 为空（例如回环地址）时保留旧值。
- 文件只有 `baton-net` 读写（`PeerStore`），进程内共享一个实例，所有修改经同一把锁。
- 取消配对：从档案删除，断开现有连接，并通知协调器忘掉该设备（`Event::PeerRemoved`）。

## 7. 发现与拨号

- **广播**：`mdns-sd` 注册 `_deskbaton._tcp.local.`，实例名为设备名称，TXT 为 `fp=<指纹十六进制>`、`v=<协议版本>`。
- **浏览**：解析到的服务按指纹归档；自己的指纹忽略。服务消失时移除。
- **自动拨号的对象**：档案里的设备；配置了 `network.token` 时还有 `[[peers]]` 里写了 `address` 的设备。未配对、也没有令牌的设备不自动拨号，只出现在面板的「可配对」列表里，由用户点「配对」发起一次拨号。
- **拨号地址**按顺序尝试，第一个连上即止：
  1. mDNS 按指纹解析到的地址；
  2. 档案里的 `last_ip:port`；
  3. 配置里的 `peers[].address`（按名称对应）。
- 每 2 秒检查一次未连接的对端并重试；名称较大的一方首轮晚 1 秒，减少同时互拨。
- mDNS 由 `LinkConfig.discovery` 开关控制，单测与回环测试关闭。

## 8. 唤醒信息

- agent 启动时把档案里的设备作为「已知、离线」交给协调器，所以对端一直睡着、本机刚开机也能唤醒它。
- 协调器取 `wol_mac`：档案（含握手刷新）优先，配置 `peers[].wol_mac` 兜底。
- `WakeTarget` 增加 `last_ip`；`WolWaker` 同时发往 `255.255.255.255`、本机各 IPv4 网卡的子网广播地址、`last_ip`，端口均为 UDP 9。任一目标发送成功即算 `Sent`。
- 交棒目标：配置里的对端优先（按配置顺序，在线的先），其次档案里的设备（按名称排序，在线的先）。

## 9. 进程合并

- `baton-agent` 改为库 + 无界面程序：库里的 `Agent::start` 负责恢复睡眠设置、加载身份与档案、启动链路和协调器线程，返回句柄（事件发送端、链路、热键）。
- `deskbaton`（无界面）：调用 `Agent::start`，在主线程跑系统事件循环分发热键；支持 `--stdin`。
- `deskbaton-gui`：启动时调用 `Agent::start`，热键在 Tauri 的主线程事件循环里分发。agent 启动失败（如端口被占用、配置有误）时面板照常可用，设备卡片显示原因。
- 退出：托盘「退出」、`Ctrl+C`、`SIGTERM` 都走「发送 `Event::Shutdown` → 等协调器线程结束」，保证 `AwakeGuard` 释放后进程才退出。
- 面板与协调器各自持有一个 DDC 实例。面板切输入源仍不经交棒流程（见面板设计第 3 节）。
- 两者不要同时运行：都监听 47800，后启动的会报端口被占用。

## 10. 面板「设备」卡片

```text
┌──────────────────────────────────┐
│ 设备                 局域网 · 已开启 │
│ ┌ 配对请求 ─────────────────────┐ │
│ │ windows-desktop               │ │
│ │       482 913                 │ │  等宽大字号
│ │ 请确认两台设备显示相同数字     │ │
│ │            [拒绝] [同意]       │ │
│ └───────────────────────────────┘ │
│ ● windows-desktop  在线           │  已配对：状态点 + IP · MAC，悬停出现「移除」
│   192.168.1.20 · aa:bb:…:ff       │
│ ○ mac-mini         可配对  [配对] │  发现到、未配对
└──────────────────────────────────┘
```

- 有需要本机确认的配对请求时自动弹出面板（摆在最近一次的托盘位置）。
- 本机已同意、等对方时显示「等待对方确认…」；结束后状态行提示「已与 X 配对」或「配对未完成：原因」。
- 后端在设备、会话或连接状态变化时发 `devices-changed` 事件，前端调用 `list_devices` 取快照重新渲染。
- Tauri 命令：`list_devices`、`confirm_pairing(session, accept)`、`pair_device(fingerprint | address)`、`unpair_device(fingerprint)`。
- 协调器的提示（交棒失败等）经 `notice` 事件显示在状态行。

## 11. 代码落点

| 位置 | 内容 |
|---|---|
| `baton-core/src/config.rs` | `data_dir()`；`default_config_path()` 基于它 |
| `baton-core/src/protocol.rs` | v3：`hello` 增 `listen_port`、`auth` 可选；新增 `pair_commit` / `pair_nonce` / `pair_reveal` / `pair_pending` / `pair_accept` |
| `baton-core/src/event.rs` | `PeerInfo.ip`；`Event::PeerRemoved` |
| `baton-core/src/ports.rs` | `WakeTarget.last_ip` |
| `baton-core/src/coordinator.rs` | `with_known_peers`、目标选择、`wol_mac` 优先级 |
| `baton-net/src/identity.rs` | 身份加载 / 生成 |
| `baton-net/src/pairing.rs` | 配对码、承诺、信任判定（纯逻辑） |
| `baton-net/src/store.rs` | `PeerStore` |
| `baton-net/src/iface.rs` | 按本地 IP 查网卡 MAC（`netdev`） |
| `baton-net/src/discovery.rs` | mDNS 广播与浏览 |
| `baton-net/src/link.rs` | 握手 v3、拨号循环、配对会话、`LinkObserver`、快照与配对 API |
| `baton-power/src/wake/wol.rs` | 多目标发送 |
| `baton-agent/src/lib.rs` | `Agent::start` |
| `baton-gui/src/agent.rs` | 面板内的 agent、热键、设备命令 |

## 12. 测试

- **单测**：配对码对称（两端算出同一数）、任一指纹或随机数变化则改变；承诺校验；信任判定表逐行；档案读写往返、按名称替换、未知字段报错；拨号地址合并顺序；`WolWaker` 目标列表（去重、含单播）；协调器在对端从未上线时用档案里的 MAC 唤醒、档案优先于配置。
- **回环**：
  - 令牌路径：首次连接自动写入双方档案；之后去掉令牌仍能连上；
  - 无令牌、无界面：被拒；
  - 交互配对：两端用测试观察者自动同意，验证双方看到相同配对码、写入档案、上线；一方拒绝则另一方不上线、档案不变；
  - 指纹不符：档案里同名设备换了证书时不被当作已配对；
  - 原有的双向交棒回环测试改用档案（不再依赖令牌）；
  - mDNS：本机两条链路互相发现、只凭 mDNS 地址连上已配对设备。依赖网卡组播，CI 不一定具备，标为 `#[ignore]`，用 `cargo test -p baton-net -- --ignored` 手动运行。
- **硬件验收清单**（手动）：
  1. 两台机器各开面板，一方点「配对」，两边显示相同 6 位数字；都同意后互相显示在线；两边重启后不再要求配对，自动连上；
  2. Windows 防火墙开启时，两边面板都能在「可配对」里看到对方（mDNS）；
  3. 一方拒绝，另一方提示配对未完成，双方档案都没有新增；
  4. 配置里不写 `wol_mac`：Mac 合盖睡眠后，从 Windows 交棒能凭档案唤醒 Mac；
  5. 只运行面板（不运行 `deskbaton`）时，热键交棒与第 1 项的连接都正常。

## 13. 风险

| 风险 | 应对 |
|---|---|
| 协议升到 v3，两端版本不一致 | 握手提示「协议版本不一致」，两端同时更新 |
| mDNS 被防火墙或路由器拦截 | 档案地址和配置地址兜底；面板仍可通过配置里的地址发起配对 |
| 私钥文件被其他程序读取 | 权限仅当前用户；泄露时删除 `identity/` 重新生成，并在对端移除后重新配对 |
| 面板与协调器在同一进程里并发 DDC | 与此前两个进程并发的情况相同；面板切输入源并入交棒流程另列 TODO |
| Tauri 接管主线程后热键行为变化 | 硬件验收第 5 项 |
