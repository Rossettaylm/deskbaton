# AGENTS.md

DeskBaton（交棒）：一台显示器、多台电脑，一键把画面和键鼠交给另一台电脑。Rust workspace，GPL-3.0-or-later。

设计依据：`docs/superpowers/specs/2026-10-16-deskbaton-design.md`。改行为先改设计文档，再改代码。

## 产品不变量

- 画面和键鼠始终在同一台机器（持棒方）。只有持棒方响应热键、发 DDC 命令。
- 不假设显示器接受非活跃输入口的 DDC 命令。
- 持棒判定：`get_input()` 等于本机 `display.input_code`。
- Mac 保持唤醒只能经 `PowerControl::stay_awake` 返回的 `AwakeGuard`，默认在对端在线期间保持。不得永久修改系统睡眠策略；guard 释放和 agent 启动时都要恢复。
- 交棒附带能力（剪贴板等）失败只记日志，不得阻断或拖慢交棒。
- 剪贴板只在交棒时同步；跳过带敏感标记的内容；超过 32 MiB 跳过。开关在面板「设置」，写入 `state.toml`。
- 网络只限局域网：TLS + 证书指纹固定，不暴露到公网。
- 唤醒配件只执行唤醒，不接受任意按键输入，必须校验配对令牌。

## 架构

- `baton-core` 不依赖任何平台 crate 或系统 API。外部能力定义为 `baton_core::ports` 中的 trait，由平台 crate 实现，`baton-agent` 负责组装。
- 扩展只通过新增实现接入：`InputShareBackend`、`Waker` + `WakeMethod`、`ChannelId` + `HandoffParticipant`。不为新能力改交棒流程。
- 平台代码按 `#[cfg(target_os = "...")]` 分模块。未实现的能力返回 `Error::Unsupported`，禁止 `todo!`、`unimplemented!`、`panic!`。
- macOS 私有 API 和 ObjC 只放在 `baton-macos-sys/native`，Rust 侧只做 FFI 声明。
- 帧格式：`len: u32 BE | channel: u8 | kind: u8 | payload`。通道 0 为控制通道（JSON `Message`）。控制帧优先，数据按 64 KiB 分块。
- 改线上格式必须升 `PROTOCOL_VERSION`，并更新格式稳定性测试。
- 配置为 TOML，`deny_unknown_fields`。新增字段必须有默认值。
- 未验证的硬件事实（输入源编码、KVM 行为等）做成配置项，不写死。

## 代码

- Rust 2024；版本、edition、license、lints 从 workspace 继承。
- 依赖用到时再加，统一声明在 `[workspace.dependencies]`。许可证必须兼容 GPL-3.0；GPL-2.0 的 Deskflow 只能以独立进程调用。
- `unsafe` 只用于 FFI，每处写 `// SAFETY:`。
- 错误统一用 `baton_core::Error`；面向用户的文案用中文。
- 注释只写代码表达不了的约束，不写来源、过程或改动理由。
- 核心逻辑用假实现做单测；依赖硬件的行为写进设计文档的验收清单。

## 推进

- 工作清单在 `TODO.md`，按优先级从上到下推进，一次只做一项。
- 标 🔧 的项依赖硬件或特定环境：当前环境做不了就跳过，留给用户实测，不阻塞后续项。
- 做完一项：通过「提交前」的检查后，在 `TODO.md` 打勾，打勾与代码放在同一个提交里。🔧 项由用户确认结果后才打勾。
- 做的过程中发现的新工作，追加到 `TODO.md` 的对应优先级，不顺手做。
- 实测结论和设计不符时，先改设计文档和 `TODO.md`，再继续。

## 提交前

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI 在 macOS / Windows / Linux 上执行同样的检查。

## Git

- 提交信息：Conventional Commits 前缀 + 中文描述，如 `feat: 实现 Windows DDC 读写`。
- 身份只在仓库本地配置：`Rossettaylm <2441197035@qq.com>`。不修改全局 git 配置。
- 调研结论放 `docs/research/`，设计放 `docs/superpowers/specs/`。
