# DeskBaton 剪贴板同步设计

日期：2026-10-17　状态：已确认

主设计见 `2026-10-16-deskbaton-design.md`。本机设置见 `2026-10-17-panel-settings-design.md`。冲突时以本文为准。

本文落实 M2 的「交棒时同步系统剪贴板」。实时同步、文件列表、视频、HTML / RTF 仍属 M3，另立文档。

## 1. 目标

交棒时把持棒方当前系统剪贴板里的文本或图片写到接棒方的系统剪贴板。对端其它剪贴板软件能看到刚写入的内容。

## 2. 范围

**做**

- 用 `clipboard-rs` 读写系统剪贴板（文本、PNG 图片）
- 只在交棒时同步（`HandoffParticipant`，通道 `1`）
- 防回传、超过 32 MiB 跳过、敏感约定标记跳过
- 面板「本机」卡片一个「剪贴板」开关；开关写入 `state.toml`
- 失败不阻断、不拖慢交棒

**不做**

- 实时同步、变更监听
- HTML / RTF / 文件列表 / 视频
- 用户可见的 `config.toml` 字段或体积上限旋钮
- 回写 `config.toml`

## 3. 架构

协调器已有交棒参与者：切输入源前并行 `pack`，经通道 `1` 发送；对端收到完整数据后 `unpack`。本能力只补上这个参与者，不改交棒流程，不升 `PROTOCOL_VERSION`。

```text
持棒方 pack                          接棒方 unpack
clipboard-rs 读系统剪贴板            clipboard-rs 写系统剪贴板
        ↓                                    ↑
   ClipboardSync  ── TLS 通道 1 ──►  ClipboardSync
        ↑
   ClipboardAccess（baton-core 端口）
```

| 位置 | 职责 |
|---|---|
| `baton-core` | `ClipboardAccess`、`ClipboardContent`、`ClipboardRead`；`LocalState.clipboard_sync` |
| `baton-clipboard` | `clipboard-rs` 实现访问；`ClipboardSync` 实现 `HandoffParticipant` |
| `baton-agent` | 始终注入 `ClipboardSync`；开关为进程内 `AtomicBool` |
| `baton-gui` | 「本机」卡片开关；只写 `state.toml` |

`baton-core` 不依赖 `clipboard-rs` 或系统 API。

## 4. 内容

读系统剪贴板时只带一种，顺序：

1. 带敏感约定标记 → 整份跳过，不提示（避免暴露「刚复制了密码」）。
2. 有图片 → `image/png`。
3. 否则有文本 → `text/plain`（UTF-8）。
4. 都没有、读失败、或超过 32 MiB → 本次不带。超限时经 `Notifier` 提示「剪贴板过大，未同步」。

同时有图和字时只传图。HTML / RTF / 文件列表不读不写。

线上仍是一条 `Payload`：

```text
Payload { format: "text/plain" | "image/png", data: 原始字节 }
```

接棒方按 `format` 写回系统剪贴板。不认识的 `format` 丢掉并记日志。

防回传：记住最近一次发出或写入的内容哈希（`format` + 字节的 SHA-256）。下次 `pack` 读到同一份就返回 `None`。

敏感标记只认常见约定，用 `clipboard-rs` 的 `available_formats` 判断：

| 平台 | 类型名 |
|---|---|
| macOS | `org.nspasteboard.ConcealedType` |
| Windows | `ExcludeClipboardContentFromMonitorProcessing` |

认不到就当普通内容。

## 5. 设置

用户只在面板「本机」里改。`state.toml` 是程序自己的持久化，用户不用看见、不用手改。不往 `Config` / `config.toml` 加字段。

| 项 | 真相 | 默认 |
|---|---|---|
| 切换时同步剪贴板 | `state.toml` 的 `clipboard_sync` | 开 |
| 体积上限 | 代码常量 `MAX_CLIPBOARD_BYTES` = 32 MiB | 不进面板、不进文件 |

`LocalState` 增加 `clipboard_sync: bool`，缺省 `true`，继续 `deny_unknown_fields`。写入走现有读-改-写。

本机卡片在「通知」下面加一行「剪贴板」。拨动后先落盘，再立刻翻转 `ClipboardSync` 持有的 `AtomicBool`。`pack` 和 `unpack` 都看这个开关：关掉后本机既不带走、也不写入对端送来的内容。已经在路上的那一次传完仍会到，到了也不写。

无界面 `deskbaton` 启动时读同一个字段，没有开关可拨。

agent 启动失败时，面板仍从 `state.toml` 读写该开关。

旧手写 `[clipboard]` 若出现在 `config.toml`，`deny_unknown_fields` 会拒读。本轮不兼容这条未落地的示例，也不做回退。`config.example.toml` 不写剪贴板。

面板设置设计第 2 节「剪贴板……的面板编辑」不再适用。

## 6. 端口

```rust
pub enum ClipboardContent {
    Text(String),
    ImagePng(Vec<u8>),
}

pub enum ClipboardRead {
    Empty,
    Sensitive,
    Content(ClipboardContent),
}

pub trait ClipboardAccess: Send + Sync {
    fn read(&self) -> Result<ClipboardRead>;
    fn write(&self, content: &ClipboardContent) -> Result<()>;
}
```

系统实现每次读写新建 `clipboard-rs` 的上下文，不跨线程长期持有。打不开系统剪贴板时返回 `Error::Unsupported("剪贴板")`；`ClipboardSync` 把它当成本次不带。

## 7. 失败

读、传、写、敏感检测、超限，全部不阻断交棒。该提示的走现有 `Notifier`（受「通知」开关约束）。对端中止交棒时，未交付的 transfer 照现有逻辑丢掉。

| 场景 | 处理 |
|---|---|
| 系统剪贴板读失败 / 空 / `Unsupported` | 本次不带，只记日志 |
| 敏感标记 | 不带，不提示 |
| 超过 32 MiB | 不带，提示「剪贴板过大，未同步」 |
| 传输失败或对端关掉同步 | 发送方只记日志；接收方不写 |
| 写入系统剪贴板失败 | 日志 + 提示「未能写入剪贴板」 |
| 不认识的 `format` | 丢掉，只记日志 |

## 8. 测试

- `baton-clipboard` 用假 `ClipboardAccess`：文本 / 图片各走一遍、防回传、超限、敏感跳过、开关关闭时 `pack` 和 `unpack` 都不碰剪贴板。
- `state.toml`：`clipboard_sync` 缺省为开、未知字段拒绝、读-改-写不丢其它偏好。
- 协调器现有参与者测试不动。回环集成仍用假参与者，CI 不碰真实系统剪贴板。
- CI：既有 `fmt` / `clippy -D warnings` / `test`。

硬件验收（手动，🔧，用户确认后才打勾）：

1. 本机复制一段文字，交棒后对端能粘贴，系统剪贴板历史里也能看到。
2. 本机复制一张图片（截图），交棒后对端能粘贴为图片。
3. 关掉「剪贴板」再交棒，对端剪贴板不变。
4. 密码管理器复制（带敏感标记）后交棒，对端不应出现该密码。
5. 交棒过程中剪贴板失败或超限，画面和键鼠仍能切过去。

## 9. 风险

| 风险 | 应对 |
|---|---|
| `clipboard-rs` 比 `arboard` 使用面小 | 文本 / PNG 路径用假实现单测；像素格式对不齐记进验收，首版不加格式回退 |
| 敏感 UTI / 格式名各平台不统一 | 只认上表两行；提供关闭开关 |
| Windows 剪贴板同一时刻只能打开一次 | 每次读写新建上下文，不长期持有 |
| 学习线程或其它偏好写入冲掉开关 | 统一读-改-写；单测「改编码不丢 clipboard_sync」 |
