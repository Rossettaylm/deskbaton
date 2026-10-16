# DeskBaton（交棒）

一台显示器、多台电脑，按一个热键，把画面和键鼠一起交给另一台电脑——包括唤醒合盖睡眠中的 MacBook。

> 状态：早期开发，仅有工程骨架，尚不可用。

## 工作方式

- **切画面**：通过 DDC/CI 切换显示器输入源。
- **切键鼠**：键鼠插在显示器 USB 口上时由显示器内置 KVM 跟随；也预留了纯软件转发模式。
- **唤醒**：对端睡眠时按配置依次尝试 Wake-on-LAN、HID 唤醒配件、提示用户。

## 代码结构

| crate | 职责 |
|---|---|
| `baton-core` | 领域类型、协议、配置、端口接口、交棒协调器（与平台无关） |
| `baton-display` | DDC/CI 输入源控制（Windows / macOS） |
| `baton-power` | 本机睡眠控制、唤醒对端 |
| `baton-input` | 键鼠路由：显示器 KVM 模式 / 软件模式 |
| `baton-net` | 局域网发现、配对、加密传输 |
| `baton-macos-sys` | macOS 原生胶水（ObjC） |
| `baton-agent` | 常驻程序 `deskbaton` |
| `baton-cli` | 调试工具 `deskbaton-cli` |

## 文档

- [开源生态调研](docs/research/2026-10-16-kvm-input-switch-landscape.md)
- [首版设计](docs/superpowers/specs/2026-10-16-deskbaton-design.md)

## 开发

```bash
cargo test --workspace
cargo run -p baton-cli -- --help
```

## 许可证

[GPL-3.0-or-later](LICENSE)
