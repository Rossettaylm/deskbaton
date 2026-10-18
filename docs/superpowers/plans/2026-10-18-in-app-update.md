# 程序内自动更新 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 安装版面板启动检查 GitHub Release，用户确认后验签覆盖安装并重启。

**Architecture:** 只动 `baton-gui`。`UpdateSlot` 管状态；`tauri-plugin-updater` 取 `latest.json` 并下载验签；开发版 `install_build_at` 为假则不发网络。tag `vX.Y.Z` 才打带签名的更新包并发 Release。

**Tech Stack:** Tauri 2、`tauri-plugin-updater`、minisign、GitHub Release、`tauri-action`。

## Global Constraints

- 不改交棒、配对、剪贴板、`PROTOCOL_VERSION`。
- 不把更新做成 `baton-core` 端口。
- 前端不加 npm；Rust 命令 + 现有 `invoke`。
- 不引进 `tauri-plugin-process`；用 `AppHandle::restart()`。
- 私钥不进仓库；`createUpdaterArtifacts` 仓库里为 false。
- 错误 `baton_core::Error`；用户文案中文。
- Linux CI 继续 `--exclude baton-gui`。
- 用户说提交时才用 `scripts/ship.sh`，任务内不要 `git commit`。

---

## File Structure

- Modify: `crates/baton-gui/src/startup.rs` — `install_build_at`
- Create: `crates/baton-gui/src/update.rs` — 槽位、文案、检查/安装
- Modify: `crates/baton-gui/src/commands.rs` — 三个 Tauri 命令
- Modify: `crates/baton-gui/src/main.rs` — 插件、manage、启动检查
- Modify: `crates/baton-gui/ui/{index.html,main.js,style.css}` — 本机「版本」行
- Modify: `crates/baton-gui/tauri.conf.json` — updater 公钥 / endpoint
- Modify: `crates/baton-gui/src/bundle_config.rs` — 锁住配置与 pack.yml
- Modify: `.github/workflows/pack.yml` — 版本文件名、tag Release
- Modify: `Cargo.toml` / `crates/baton-gui/Cargo.toml` — `tauri-plugin-updater`
- Modify: `TODO.md`

---

### Task 1: 安装版路径判定

**Files:** `crates/baton-gui/src/startup.rs`

**Produces:** `pub fn install_build_at(exe: &Path, local_app_data: Option<&Path>) -> bool`；`pub fn is_install_build() -> bool`

- [ ] 先写失败单测（Mac `.app` 真、`target/debug` 假、Windows `Programs/DeskBaton` 真、`target/release` 假）
- [ ] `cargo test -p baton-gui install_build` 确认失败
- [ ] 实现 `install_build_at` / `is_install_build`
- [ ] 测试通过

### Task 2: UpdateSlot

**Files:** Create `crates/baton-gui/src/update.rs`

**Produces:** `UpdateSlot`、`UpdateStatus { supported, kind, version }`、`progress_text`、`check_with`

- [ ] 先写失败单测：开发版不调用 fetch；有更新只公告一次；安装中拒绝再装；进度文案
- [ ] 实现最小状态机
- [ ] 测试通过

### Task 3: 配置、密钥、pack.yml

**Files:** `tauri.conf.json`、`bundle_config.rs`、`pack.yml`、`Cargo.toml`

- [ ] 先写失败单测：endpoint、公钥非空、`createUpdaterArtifacts==false`、pack 有 tag 与版本变量、无写死 `0.0.1` 文件名
- [ ] `tauri signer generate` 公钥写入配置；私钥只给用户放 Secrets
- [ ] 改 pack.yml：读 workspace 版本；tag 校验；`--config` 打开更新器产物；发 Release
- [ ] 测试通过

### Task 4: 命令、启动检查、面板

**Files:** `commands.rs`、`main.rs`、`ui/*`

- [ ] 注册插件与 `UpdateSlot`；安装版 spawn `check_update`
- [ ] `update_status` / `check_update` / `install_update`
- [ ] 本机卡片「版本」行；事件 `update-available` / `update-progress`
- [ ] `cargo test -p baton-gui`、`cargo clippy -p baton-gui --all-targets -- -D warnings`
