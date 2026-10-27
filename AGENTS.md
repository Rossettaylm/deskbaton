# AGENTS.md

DeskBaton（交棒）：局域网内多台电脑共用一台或多台显示器，一键把画面和键鼠交给另一台电脑，并在电脑之间传输信息。Rust workspace，GPL-3.0-or-later。

设计依据：`docs/superpowers/specs/2026-10-16-deskbaton-design.md`。改行为先改设计文档，再改代码。

## 产品守则

- 用户画像：同一局域网内有多台主机（Windows、macOS、Linux 任意组合），接一台或多台显示器。
- 产品用途：在局域网内切换和控制显示器输入，并在多台主机之间传输信息。
- 所有产品取舍都以这个画像为依据：先保证安全，再尽量简化用户的操作。

## 产品不变量

- 画面和键鼠始终在同一台机器（持棒方）。只有持棒方响应热键、发 DDC 命令。
- 不假设显示器接受非活跃输入口的 DDC 命令。
- 持棒判定：`get_input()` 等于本机 `display.input_code`。
- Mac 保持唤醒只能经 `PowerControl::stay_awake` 返回的 `AwakeGuard`，默认在对端在线期间保持。不得永久修改系统睡眠策略；guard 释放和 agent 启动时都要恢复。
- 交棒附带能力失败只记日志，不得阻断或拖慢交棒。剪贴板已改为实时同步，不再随交棒 `pack`。
- 剪贴板实时发给已配对对端；对端离线时进该对端发件箱，上线后按序送出。不转发收到的内容；跳过敏感标记；超过 32 MiB 跳过。总开关写入 `state.toml`。面板只在设备行显示待发条数，不展示内容，不展示未发列表。
- 软件调光（gamma）只在显示器明确不支持 DDC 亮度时使用，不持久化：亮度回到 100% 或面板退出时复原；上次异常退出留下的调光在下次启动时复原。
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

改完代码先自验 Windows，红了自己修再跑，不必等用户换机器：

- `scripts/check-windows.sh`：在 Mac 上对 Windows 目标跑 clippy（需 `brew install llvm`、`cargo install --locked cargo-xwin`）。
- `scripts/verify-ci.sh`：把工作区快照推到 `agent/verify-<分支>`，等三平台 CI；涉及 Windows 运行时、安装器或打包时加 `--pack`，再等安装包构建和 Windows 安装冒烟测试（`scripts/smoke-windows.ps1`）。

用户说提交时，agent 只写 Conventional Commits 信息（前缀 + 中文，如 `feat: 实现 Windows DDC 读写`），然后跑 `scripts/ship.sh`。不要自己 `git add` / `git commit` / `git push` / `gh workflow run`。

```bash
scripts/ship.sh "$(cat <<'EOF'
feat: 标题

正文（可选）
EOF
)"
```

脚本会：`cargo fmt --all`、`clippy -D warnings`（Mac 上再跑 `check-windows.sh`，缺工具只警告）、`cargo test --workspace`，暂存全部改动并提交，`git push -u origin HEAD`，触发 `pack.yml`，确认安装包 artifact（`DeskBaton-0.0.1-macos-aarch64.dmg`、`DeskBaton-0.0.1-windows-x64-setup.exe`）已排队。CI 在 macOS / Windows / Linux 上另做同样的检查。`ship.sh` 不打 tag。

这次提交改了 workspace `version` 时，`ship.sh` 成功推上 `main` 之后必须再打 annotated tag 并推送，才会编 GitHub Release（安装包 + `latest.json`，供程序内更新）。只推 `main` 只有 7 天 artifact，没有 Release。tag 名是 `v` 加 workspace 版本，必须与 `Cargo.toml` 里的 `version` 一致，例如版本 `0.1.1` 则：

```bash
git tag -a v0.1.1 -m "DeskBaton v0.1.1"
git push origin v0.1.1
```

tag 推送会再触发 `pack.yml`。版本没变的提交不打 tag。

## Git

- agent 只能经 `scripts/verify-ci.sh` 推送 `agent/` 前缀的验证分支；这些分支每次强推、不合入。
- 身份只在仓库本地配置：`Rossettaylm <2441197035@qq.com>`。不修改全局 git 配置。
- 调研结论放 `docs/research/`，设计放 `docs/superpowers/specs/`。
