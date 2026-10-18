# Installer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Mac / Windows 用安装包分发 DeskBaton，CI 打出 `.dmg` 与 NSIS，安装后预置登录自启；Mac 安装版用 `SMAppService` 助手执行两条 `pmset`。

**Architecture:** 用户面仍是 `deskbaton-gui`。Tauri 打开 bundle。Windows 用 NSIS 钩子写 HKCU Run。macOS 从 `.app` 启动时走 `SMAppService` 登录项和特权助手；裸二进制维持 LaunchAgent / sudoers。`baton-sleep-helper` 只校验 argv 并 `exec` pmset；安装版经 XPC 调它（launchd 以 root 拉起）。

**Tech Stack:** Tauri 2 bundle、NSIS hooks、`SMAppService`、LaunchDaemon + XPC、GitHub Actions `tauri-action`。

## Global Constraints

- 不改交棒、配对、剪贴板、`PROTOCOL_VERSION`。
- 安装包不带 `deskbaton`、`deskbaton-cli`。
- 不做 Linux 安装器 / 面板；Linux CI 继续 `--exclude baton-gui`。
- 主程序留在登录会话，不做 Windows Service。
- `unsafe` 只用于 FFI，每处写 `// SAFETY:`。
- 错误用 `baton_core::Error`；用户文案中文。
- 版本 `0.0.1`；`identifier` 保持 `io.github.rossettaylm.deskbaton.panel`。
- 助手只允许 `/usr/bin/pmset -a disablesleep 1` 与 `... 0`。
- 提交前：`cargo fmt --all`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- 提交信息：Conventional Commits 前缀 + 中文。

---

## File Structure

- Create: `crates/baton-gui/src/bundle_config.rs` — 读 `tauri.conf.json` 的单测
- Modify: `crates/baton-gui/tauri.conf.json` — 打开 bundle
- Create: `crates/baton-gui/windows/hooks.nsh` — Run 键
- Modify: `crates/baton-gui/src/startup.rs` — 安装版 SMAppService 登录项
- Create: `crates/baton-sleep-helper/` — argv 放行 + exec pmset + XPC 服务端
- Create: `crates/baton-gui/macos/LaunchDaemons/io.github.rossettaylm.deskbaton.sleep.plist`
- Create: `crates/baton-macos-sys/native/service.m` — SMAppService + XPC 客户端
- Modify: `crates/baton-power/src/sleep_helper.rs` — 安装版走助手
- Create: `.github/workflows/pack.yml`
- Delete: `.github/workflows/windows-build.yml`

---

### Task 1: 打开 Tauri bundle 并用单测锁住字段

**Files:**
- Create: `crates/baton-gui/src/bundle_config.rs`
- Modify: `crates/baton-gui/src/main.rs`（`mod bundle_config;`）
- Modify: `crates/baton-gui/tauri.conf.json`
- Modify: `crates/baton-gui/Cargo.toml`（dev-dep `serde_json`）

**Interfaces:**
- Consumes: 无
- Produces: `tauri.conf.json` 的 `productName`=`DeskBaton`，`bundle.active`=true，`targets`=`["dmg","nsis"]`，`nsis.installMode`=`currentUser`；`externalBin` 指向 `binaries/deskbaton-sleep-helper`（文件本任务可先不存在，bundle 单测只读 JSON）

- [ ] **Step 1: 写失败的单测**

`crates/baton-gui/src/bundle_config.rs`：

```rust
//! 锁住安装器规格要求的 Tauri bundle 字段。

#[cfg(test)]
mod tests {
    #[test]
    fn bundle_matches_installer_spec() {
        let raw = include_str!("../tauri.conf.json");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        assert_eq!(v["productName"], "DeskBaton");
        assert_eq!(v["identifier"], "io.github.rossettaylm.deskbaton.panel");
        assert_eq!(v["bundle"]["active"], true);
        let targets = v["bundle"]["targets"].as_array().unwrap();
        assert!(targets.iter().any(|t| t == "dmg"));
        assert!(targets.iter().any(|t| t == "nsis"));
        assert_eq!(targets.len(), 2);
        assert_eq!(v["bundle"]["windows"]["nsis"]["installMode"], "currentUser");
        assert_eq!(
            v["bundle"]["windows"]["nsis"]["installerHooks"],
            "windows/hooks.nsh"
        );
        let bins = v["bundle"]["externalBin"].as_array().unwrap();
        assert!(
            bins.iter()
                .any(|b| b == "binaries/deskbaton-sleep-helper")
        );
    }
}
```

`main.rs` 顶部加 `mod bundle_config;`。`Cargo.toml` 加：

```toml
[dev-dependencies]
serde_json.workspace = true
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-gui bundle_matches_installer_spec -- --nocapture`

Expected: FAIL，`productName` 仍是 `DeskBaton Panel` 或缺字段。

- [ ] **Step 3: 改 `tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "DeskBaton",
  "identifier": "io.github.rossettaylm.deskbaton.panel",
  "build": {
    "frontendDist": "ui"
  },
  "app": {
    "withGlobalTauri": true,
    "macOSPrivateApi": true,
    "windows": [
      {
        "label": "panel",
        "url": "index.html",
        "title": "DeskBaton",
        "width": 360,
        "height": 480,
        "resizable": false,
        "decorations": false,
        "transparent": true,
        "alwaysOnTop": true,
        "skipTaskbar": true,
        "visible": false,
        "focus": false
      }
    ],
    "security": {
      "csp": "default-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; script-src 'self'"
    }
  },
  "bundle": {
    "active": true,
    "targets": ["dmg", "nsis"],
    "icon": ["icons/icon.png", "icons/icon.ico"],
    "externalBin": ["binaries/deskbaton-sleep-helper"],
    "macOS": {
      "minimumSystemVersion": "13.0"
    },
    "windows": {
      "nsis": {
        "installMode": "currentUser",
        "installerHooks": "windows/hooks.nsh"
      }
    }
  }
}
```

- [ ] **Step 4: 再跑测试**

Run: `cargo test -p baton-gui bundle_matches_installer_spec`

Expected: PASS（`hooks.nsh` 本任务可以是空文件，避免以后 Tauri 校验失败则下一步立刻补）

同时建占位：

`crates/baton-gui/windows/hooks.nsh`：

```nsh
; Run 键由 Task 2 写入。
```

`crates/baton-gui/binaries/.gitkeep` 空目录说明。`.gitignore` 在 `crates/baton-gui/binaries/` 忽略带 triple 后缀的二进制，保留 `.gitkeep`。

- [ ] **Step 5: Commit**

```bash
git add crates/baton-gui/src/bundle_config.rs crates/baton-gui/src/main.rs crates/baton-gui/tauri.conf.json crates/baton-gui/Cargo.toml crates/baton-gui/windows/hooks.nsh crates/baton-gui/binaries/.gitkeep
git commit -m "$(cat <<'EOF'
feat: 打开 Tauri 安装包配置

分发改为 DeskBaton 的 dmg/nsis，不再默认打成裸面板名。
EOF
)"
```

---

### Task 2: NSIS 钩子写 / 删 Run 键

**Files:**
- Modify: `crates/baton-gui/windows/hooks.nsh`

**Interfaces:**
- Consumes: Task 1 的 `installerHooks` 路径
- Produces: 安装后 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` 值名 `DeskBaton` = `"$INSTDIR\DeskBaton.exe"`；卸载时删除

- [ ] **Step 1: 用脚本锁住钩子文本（当单测）**

在 `bundle_config.rs` 的 tests 里追加：

```rust
    #[test]
    fn nsis_hooks_write_current_user_run_key() {
        let hooks = include_str!("../windows/hooks.nsh");
        assert!(hooks.contains("NSIS_HOOK_POSTINSTALL"));
        assert!(hooks.contains("NSIS_HOOK_PREUNINSTALL"));
        assert!(hooks.contains("Software\\Microsoft\\Windows\\CurrentVersion\\Run"));
        assert!(hooks.contains("DeskBaton"));
        assert!(hooks.contains("$INSTDIR\\DeskBaton.exe"));
        assert!(hooks.contains("WriteRegStr HKCU"));
        assert!(hooks.contains("DeleteRegValue HKCU"));
    }
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-gui nsis_hooks_write_current_user_run_key`

Expected: FAIL（占位文件没有这些串）

- [ ] **Step 3: 写入钩子**

```nsh
!macro NSIS_HOOK_POSTINSTALL
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton" '"$INSTDIR\DeskBaton.exe"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "DeskBaton"
!macroend
```

- [ ] **Step 4: 再跑测试**

Run: `cargo test -p baton-gui --lib`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/baton-gui/windows/hooks.nsh crates/baton-gui/src/bundle_config.rs
git commit -m "$(cat <<'EOF'
feat: NSIS 安装后写入开机启动

当前用户安装完即登录自启，卸载时去掉 Run 键。
EOF
)"
```

---

### Task 3: 识别 `.app` 包，安装版登录项走 SMAppService

**Files:**
- Modify: `crates/baton-gui/src/startup.rs`
- Create: `crates/baton-macos-sys/native/service.m`
- Modify: `crates/baton-macos-sys/native/deskbaton_native.h`
- Modify: `crates/baton-macos-sys/src/lib.rs`
- Modify: `crates/baton-macos-sys/build.rs`（加入 `service.m`，链 `ServiceManagement`）

**Interfaces:**
- Consumes: 现有 `startup::{is_enabled,set_enabled,refresh_path}`
- Produces:
  - `pub fn exe_is_bundled(exe: &Path) -> bool`
  - FFI：`deskbaton_login_item_status() -> i32`（0 未登记，1 已启用，2 待用户允许，其它为错误）
  - FFI：`deskbaton_login_item_set(enabled: i32) -> i32`
  - 安装版 `imp` 调 FFI；开发版维持 LaunchAgent

- [ ] **Step 1: 写 `exe_is_bundled` 失败测试**

在 `startup.rs` 的测试模块（文件末尾新增，对所有平台编译）：

```rust
#[cfg(test)]
mod tests {
    use super::exe_is_bundled;
    use std::path::Path;

    #[test]
    fn bundled_when_an_ancestor_is_app() {
        assert!(exe_is_bundled(Path::new(
            "/Applications/DeskBaton.app/Contents/MacOS/DeskBaton"
        )));
        assert!(!exe_is_bundled(Path::new(
            "/Users/me/Code/deskbaton/target/debug/deskbaton-gui"
        )));
        assert!(!exe_is_bundled(Path::new(
            r"C:\Users\me\AppData\Local\Programs\DeskBaton\DeskBaton.exe"
        )));
    }
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-gui bundled_when_an_ancestor_is_app`

Expected: FAIL，`exe_is_bundled` 未定义。

- [ ] **Step 3: 实现路径判断，并把 macOS `imp` 按是否打包分流**

在 `startup.rs` 的 `current_exe` 旁：

```rust
pub fn exe_is_bundled(exe: &Path) -> bool {
    exe.ancestors().any(|p| {
        p.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
    })
}

pub fn running_bundled() -> bool {
    current_exe().is_ok_and(|p| exe_is_bundled(&p))
}
```

macOS `imp::is_enabled` / `enable` / `disable`：

```rust
    pub fn is_enabled() -> Result<bool> {
        if super::running_bundled() {
            return sm_enabled();
        }
        Ok(plist_path()?.is_file())
    }

    pub fn enable(exe: &Path) -> Result<()> {
        if super::exe_is_bundled(exe) {
            return sm_set(true);
        }
        // 现有 LaunchAgent 写入 + bootstrap，保持不动
        ...
    }

    pub fn disable() -> Result<()> {
        if super::running_bundled() {
            return sm_set(false);
        }
        // 现有 bootout + 删 plist
        ...
    }

    fn sm_enabled() -> Result<bool> {
        // SAFETY: 无参数。
        match unsafe { baton_macos_sys::deskbaton_login_item_status() } {
            1 | 2 => Ok(true),
            0 => Ok(false),
            other => Err(Error::Config(format!("无法读取登录项：{other}"))),
        }
    }

    fn sm_set(on: bool) -> Result<()> {
        // SAFETY: 只传 0/1。
        let rc = unsafe { baton_macos_sys::deskbaton_login_item_set(i32::from(on)) };
        if rc == baton_macos_sys::OK {
            Ok(())
        } else {
            Err(Error::Config("无法登记登录项".into()))
        }
    }
```

`deskbaton_native.h` 追加：

```c
int32_t deskbaton_login_item_status(void);
int32_t deskbaton_login_item_set(int32_t enabled);
int32_t deskbaton_helper_register(void);
int32_t deskbaton_helper_set_sleep_disabled(int32_t disabled);
```

返回值：status 用 `0` 未登记、`1` 已启用、`2` 待允许；`set` / `helper_*` 用 `DESKBATON_OK` 或负数。待允许视为「已有我们登记的项」（开关为开），与规格「开关为开 = 已有我们写的那条登录项」一致。

`service.m`：

```objc
#import "deskbaton_native.h"
#import <ServiceManagement/ServiceManagement.h>

int32_t deskbaton_login_item_status(void) {
    SMAppService *svc = [SMAppService mainAppService];
    switch (svc.status) {
        case SMAppServiceStatusNotRegistered:
        case SMAppServiceStatusNotFound:
            return 0;
        case SMAppServiceStatusEnabled:
            return 1;
        case SMAppServiceStatusRequiresApproval:
            return 2;
        default:
            return DESKBATON_ERR_IO;
    }
}

int32_t deskbaton_login_item_set(int32_t enabled) {
    SMAppService *svc = [SMAppService mainAppService];
    NSError *error = nil;
    BOOL ok = enabled ? [svc registerAndReturnError:&error]
                      : [svc unregisterAndReturnError:&error];
    return ok ? DESKBATON_OK : DESKBATON_ERR_IO;
}

int32_t deskbaton_helper_register(void) {
    SMAppService *svc = [SMAppService
        daemonServiceWithPlistName:@"io.github.rossettaylm.deskbaton.sleep.plist"];
    NSError *error = nil;
    if (svc.status == SMAppServiceStatusEnabled ||
        svc.status == SMAppServiceStatusRequiresApproval) {
        return DESKBATON_OK;
    }
    return [svc registerAndReturnError:&error] ? DESKBATON_OK : DESKBATON_ERR_IO;
}

int32_t deskbaton_helper_set_sleep_disabled(int32_t disabled) {
    xpc_connection_t conn = xpc_connection_create_mach_service(
        "io.github.rossettaylm.deskbaton.sleep",
        NULL,
        XPC_CONNECTION_MACH_SERVICE_PRIVILEGED);
    if (!conn) {
        return DESKBATON_ERR_IO;
    }
    xpc_connection_set_event_handler(conn, ^(xpc_object_t event) {
        (void)event;
    });
    xpc_connection_resume(conn);
    xpc_object_t req = xpc_dictionary_create(NULL, NULL, 0);
    xpc_dictionary_set_int64(req, "disablesleep", disabled ? 1 : 0);
    xpc_object_t reply = xpc_connection_send_message_with_reply_sync(conn, req);
    int32_t rc = DESKBATON_ERR_IO;
    if (reply && xpc_get_type(reply) == XPC_TYPE_DICTIONARY) {
        rc = (int32_t)xpc_dictionary_get_int64(reply, "rc");
    }
    return rc;
}
```

`lib.rs` 的 `unsafe extern "C"` 增加对应四个函数。`build.rs` 的 file 列表加 `native/service.m`，framework 列表加 `ServiceManagement`。

`deskbaton_helper_*` 本任务只声明并实现；Task 5 才从 `sleep_helper` 调用。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p baton-gui bundled_when_an_ancestor_is_app`

Expected: PASS

Run: `cargo test -p baton-gui --lib`

Expected: PASS

macOS 上再：`cargo clippy -p baton-gui -p baton-macos-sys --all-targets -- -D warnings`

- [ ] **Step 5: Commit**

```bash
git add crates/baton-gui/src/startup.rs crates/baton-macos-sys
git commit -m "$(cat <<'EOF'
feat: 安装版用 SMAppService 登记登录项

从 .app 启动不再手写 LaunchAgent，开发版路径保持不变。
EOF
)"
```

---

### Task 4: `baton-sleep-helper` 只放行两条 pmset

**Files:**
- Create: `crates/baton-sleep-helper/Cargo.toml`
- Create: `crates/baton-sleep-helper/src/lib.rs`
- Create: `crates/baton-sleep-helper/src/main.rs`
- Modify: `Cargo.toml`（`[workspace.dependencies]` 加 `baton-sleep-helper = { path = "crates/baton-sleep-helper" }`，可选；成员已是 `crates/*`）

**Interfaces:**
- Consumes: 无
- Produces: `pub fn allowed_argv(args: &[impl AsRef<str>]) -> Option<bool>`
  - `["-a","disablesleep","1"]` → `Some(true)`
  - `["-a","disablesleep","0"]` → `Some(false)`
  - 其它 → `None`
  - `args` 不含 argv0
- `main`：`allowed_argv` 为 `Some` 则 `exec /usr/bin/pmset` 加上同样三个参数；否则以状态码 `2` 退出。无参数时进入 XPC 服务（Task 5 补完；本任务无参退出 `2`，避免未完成的服务端被 launchd 拉起空转）。非 macOS 的 `main` 打印「当前操作系统不支持」并退出 `2`。

- [ ] **Step 1: 写失败测试**

`crates/baton-sleep-helper/src/lib.rs`：

```rust
//! 只允许两条完整 pmset 命令。

pub fn allowed_argv<S: AsRef<str>>(args: &[S]) -> Option<bool> {
    let _ = args;
    None
}

#[cfg(test)]
mod tests {
    use super::allowed_argv;

    #[test]
    fn allows_only_the_two_pmset_commands() {
        assert_eq!(allowed_argv(["-a", "disablesleep", "1"]), Some(true));
        assert_eq!(allowed_argv(["-a", "disablesleep", "0"]), Some(false));
    }

    #[test]
    fn rejects_anything_else() {
        assert_eq!(allowed_argv::<&str>(&[]), None);
        assert_eq!(allowed_argv(["-a", "disablesleep"]), None);
        assert_eq!(allowed_argv(["-a", "disablesleep", "1", "extra"]), None);
        assert_eq!(allowed_argv(["-a", "disablesleep", "2"]), None);
        assert_eq!(allowed_argv(["-c", "disablesleep", "1"]), None);
        assert_eq!(allowed_argv(["-a", "sleep", "1"]), None);
        assert_eq!(allowed_argv(["/usr/bin/pmset", "-a", "disablesleep", "1"]), None);
    }
}
```

`Cargo.toml`：

```toml
[package]
name = "baton-sleep-helper"
description = "DeskBaton 特权助手：只执行两条 pmset disablesleep 命令"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true

[[bin]]
name = "deskbaton-sleep-helper"
path = "src/main.rs"

[lints]
workspace = true
```

`src/main.rs` 先空 `fn main() {}` 以便编译。

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-sleep-helper --lib`

Expected: FAIL，`allows_only_the_two_pmset_commands` 得到 `None`。

- [ ] **Step 3: 实现放行与 main**

```rust
pub fn allowed_argv<S: AsRef<str>>(args: &[S]) -> Option<bool> {
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    match args.as_slice() {
        ["-a", "disablesleep", "1"] => Some(true),
        ["-a", "disablesleep", "0"] => Some(false),
        _ => None,
    }
}
```

`main.rs`：

```rust
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match baton_sleep_helper::allowed_argv(&args) {
        Some(_) if cfg!(target_os = "macos") => exec_pmset(&args),
        Some(_) => {
            eprintln!("当前操作系统不支持 sleep-helper");
            ExitCode::from(2)
        }
        None => ExitCode::from(2),
    }
}

#[cfg(target_os = "macos")]
fn exec_pmset(args: &[String]) -> ExitCode {
    use std::os::unix::process::CommandExt;
    let err = std::process::Command::new("/usr/bin/pmset").args(args).exec();
    eprintln!("无法执行 pmset：{err}");
    ExitCode::from(1)
}

#[cfg(not(target_os = "macos"))]
fn exec_pmset(_: &[String]) -> ExitCode {
    ExitCode::from(2)
}
```

- [ ] **Step 4: 再跑测试**

Run: `cargo test -p baton-sleep-helper`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/baton-sleep-helper Cargo.toml
git commit -m "$(cat <<'EOF'
feat: 增加只允许两条 pmset 的特权助手

安装版合盖防睡经此二进制执行，拒绝其它参数。
EOF
)"
```

---

### Task 5: 安装版经助手执行 pmset，并在首次启动登记助手

**Files:**
- Create: `crates/baton-sleep-helper/src/xpc_server.rs`（仅 macOS）
- Modify: `crates/baton-sleep-helper/src/main.rs`（无参时跑 XPC）
- Create: `crates/baton-gui/macos/LaunchDaemons/io.github.rossettaylm.deskbaton.sleep.plist`
- Modify: `crates/baton-power/src/sleep_helper.rs`（`set_sleep_disabled` 分流）
- Modify: `crates/baton-gui/src/main.rs` setup：打包时调用 `deskbaton_helper_register`
- Modify: `crates/baton-macos-sys`（Task 3 已加 FFI，本任务接上）

**Interfaces:**
- Consumes: `allowed_argv`；`deskbaton_helper_register` / `deskbaton_helper_set_sleep_disabled`
- Produces: 安装版 `set_sleep_disabled` 不再 `sudo -n`；开发版不变。登记失败只警告。

XPC 约定：Mach 服务名 `io.github.rossettaylm.deskbaton.sleep`；请求字典键 `disablesleep` 为 `0|1`；回复键 `rc` 为 `0` 成功。服务端校验后再 `exec` / `Command` 调 `/usr/bin/pmset -a disablesleep 0|1`。

plist（`AssociatedBundleIdentifiers` 让系统设置显示 DeskBaton）：

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>io.github.rossettaylm.deskbaton.sleep</string>
  <key>BundleProgram</key>
  <string>Contents/MacOS/deskbaton-sleep-helper</string>
  <key>MachServices</key>
  <dict>
    <key>io.github.rossettaylm.deskbaton.sleep</key>
    <true/>
  </dict>
  <key>AssociatedBundleIdentifiers</key>
  <array>
    <string>io.github.rossettaylm.deskbaton.panel</string>
  </array>
</dict>
</plist>
```

Tauri 需把该 plist 拷进 `.app` 的 `Contents/Library/LaunchDaemons/`。在 `tauri.conf.json` 的 `bundle.resources` / macOS 配置里加入：

```json
"resources": {
  "macos/LaunchDaemons/io.github.rossettaylm.deskbaton.sleep.plist": "Library/LaunchDaemons/io.github.rossettaylm.deskbaton.sleep.plist"
}
```

若 Tauri 2 的 resources 映射不能放到 `Contents/Library`，改为 `pack.yml` 在 `tauri build` 之后、打 dmg 之前用脚本拷贝进 `.app`。优先试 resources；不行就脚本，不要同时做两套。

- [ ] **Step 1: 给 `set_sleep_disabled` 的分流写单测**

在 `sleep_helper.rs` 增加纯函数（生产代码先让测试失败）：

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepPrivileges {
    Helper,
    Sudoers,
}

pub(crate) fn privileges_for(bundled: bool) -> SleepPrivileges {
    let _ = bundled;
    SleepPrivileges::Sudoers
}

#[test]
fn installed_app_uses_helper_not_sudoers() {
    assert_eq!(privileges_for(true), SleepPrivileges::Helper);
    assert_eq!(privileges_for(false), SleepPrivileges::Sudoers);
}
```

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-power installed_app_uses_helper_not_sudoers`

Expected: FAIL，两边都是 `Sudoers`。

- [ ] **Step 3: 实现分流、XPC 服务端、登记**

```rust
pub(crate) fn privileges_for(bundled: bool) -> SleepPrivileges {
    if bundled {
        SleepPrivileges::Helper
    } else {
        SleepPrivileges::Sudoers
    }
}
```

`set_sleep_disabled`（macOS `imp`）：

```rust
    pub(crate) fn set_sleep_disabled(disabled: bool) -> Result<()> {
        match super::privileges_for(bundled()) {
            super::SleepPrivileges::Helper => set_via_helper(disabled),
            super::SleepPrivileges::Sudoers => set_via_sudo(disabled),
        }
    }

    fn bundled() -> bool {
        std::env::current_exe()
            .ok()
            .is_some_and(|p| {
                p.ancestors().any(|a| {
                    a.extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("app"))
                })
            })
    }

    fn set_via_helper(disabled: bool) -> Result<()> {
        // SAFETY: 只传 0/1。
        let rc = unsafe { sys::deskbaton_helper_set_sleep_disabled(i32::from(disabled)) };
        if rc == sys::OK {
            Ok(())
        } else {
            Err(Error::Power(
                "执行 pmset 失败，请在系统设置 → 登录项与扩展里允许 DeskBaton".into(),
            ))
        }
    }
```

`set_via_sudo` 即现有 `sudo -n pmset` 实现，错误文案不变。

需要 `use baton_macos_sys as sys;`（`macos` 模块已有）。`sleep_helper` 的 macos imp 目前没有链 sys，本任务加上 `baton-power` 已有的 macos 依赖即可。

XPC 服务端放 `baton-sleep-helper`：无参 `main` 在 macOS 调用 `run_xpc_server()`。用 `cc` 编一小段 C（`native/server.c`）或纯 Rust 调 `xpc` 都可；优先一小段 C，和现有「ObjC/C 放 native」一致。服务端收到 `disablesleep` 后走 `allowed_argv` 对应的三个参数，`Command::new("/usr/bin/pmset")` 等退出码。

`baton-gui` setup，仅 macOS：

```rust
            #[cfg(target_os = "macos")]
            if crate::startup::running_bundled() {
                // SAFETY: 无参数。
                let rc = unsafe { baton_macos_sys::deskbaton_helper_register() };
                if rc != baton_macos_sys::OK {
                    log::warn!(
                        "未能登记睡眠助手，合盖切回时可能睡眠。请在系统设置 → 登录项与扩展里允许 DeskBaton"
                    );
                }
            }
```

- [ ] **Step 4: 跑测试**

Run: `cargo test -p baton-power installed_app_uses_helper_not_sudoers`

Expected: PASS

Run: `cargo test --workspace`

Expected: PASS（Linux 排除 gui 的本地命令：`cargo test --workspace` 在本机 Mac 可含 gui）

- [ ] **Step 5: Commit**

```bash
git add crates/baton-sleep-helper crates/baton-power crates/baton-gui crates/baton-macos-sys
git commit -m "$(cat <<'EOF'
feat: 安装版经特权助手执行 pmset

合盖防睡不再依赖用户手装 sudoers，未允许助手时只警告。
EOF
)"
```

---

### Task 6: CI 打安装包，去掉裸 exe 工作流

**Files:**
- Create: `.github/workflows/pack.yml`
- Delete: `.github/workflows/windows-build.yml`
- Modify: `TODO.md`（勾上除 🔧 以外的安装器项）
- Create: `scripts/pack-artifacts.sh`（改名上传）

**Interfaces:**
- Consumes: Task 1 的 bundle 配置；Task 4 的 helper 二进制
- Produces: artifact `DeskBaton-0.0.1-macos-aarch64.dmg`、`DeskBaton-0.0.1-windows-x64-setup.exe`

pack 在 `tauri build` 前先：

```bash
cargo build --release -p baton-sleep-helper
```

仅 macos job。然后按 triple 复制到 `crates/baton-gui/binaries/deskbaton-sleep-helper-aarch64-apple-darwin`（`rustc -vV` 的 host）。Windows job 不编 helper；为满足 Tauri `externalBin`，Windows 上放一个同名占位会让 NSIS 打进无用 exe——改为 `externalBin` 只在 macOS 需要。

若 Tauri 不支持按平台 externalBin：Windows job 在 build 前生成空的 `deskbaton-sleep-helper-x86_64-pc-windows-msvc.exe` 会进安装包，违反「安装包不带多余二进制」。处理：`tauri.conf.json` 的 `externalBin` 保留；Windows pack 步骤用 `jq` 写一份临时 conf 去掉 `externalBin`，或接受 helper 的 Windows 空壳。

选定：**Windows 空壳不进包。** `pack.yml` 的 Windows job 在 tauri-action 前用脚本把 `tauri.conf.json` 的 `externalBin` 改成 `[]`（仅 CI 工作区，不提交）。Mac job 保留并复制 helper。

- [ ] **Step 1: 写 pack 工作流的结构检查（当单测）**

`crates/baton-gui/src/bundle_config.rs` 再加：

```rust
    #[test]
    fn pack_workflow_builds_named_installers() {
        let yml = include_str!("../../../.github/workflows/pack.yml");
        assert!(yml.contains("workflow_dispatch"));
        assert!(yml.contains("macos-latest"));
        assert!(yml.contains("windows-latest"));
        assert!(yml.contains("pull_request") == false || !yml.contains("\n  pull_request:"));
        assert!(yml.contains("DeskBaton-0.0.1-macos-aarch64.dmg"));
        assert!(yml.contains("DeskBaton-0.0.1-windows-x64-setup.exe"));
        assert!(yml.contains("tauri-apps/tauri-action"));
        assert!(yml.contains("crates/baton-gui"));
    }
```

`include_str` 路径从 `crates/baton-gui/src/` 到仓库根是 `../../../.github/workflows/pack.yml`。

- [ ] **Step 2: 跑测试，确认失败**

Run: `cargo test -p baton-gui pack_workflow_builds_named_installers`

Expected: FAIL，找不到文件或缺文件名。

- [ ] **Step 3: 写 `pack.yml`，删 `windows-build.yml`，改 TODO**

`.github/workflows/pack.yml`：

```yaml
name: Pack

on:
  push:
    branches: [main]
  workflow_dispatch:

env:
  CARGO_TERM_COLOR: always

jobs:
  pack:
    strategy:
      fail-fast: false
      matrix:
        include:
          - os: macos-latest
            bundle: dmg
            artifact: DeskBaton-0.0.1-macos-aarch64.dmg
          - os: windows-latest
            bundle: nsis
            artifact: DeskBaton-0.0.1-windows-x64-setup.exe
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - uses: Swatinem/rust-cache@v2
      - name: Prepare macOS helper
        if: runner.os == 'macOS'
        run: |
          cargo build --release -p baton-sleep-helper
          mkdir -p crates/baton-gui/binaries
          cp target/release/deskbaton-sleep-helper \
            crates/baton-gui/binaries/deskbaton-sleep-helper-aarch64-apple-darwin
      - name: Strip externalBin on Windows
        if: runner.os == 'Windows'
        shell: pwsh
        run: |
          $conf = 'crates/baton-gui/tauri.conf.json'
          $j = Get-Content $conf -Raw | ConvertFrom-Json
          $j.bundle.externalBin = @()
          $j | ConvertTo-Json -Depth 20 | Set-Content $conf
      - uses: tauri-apps/tauri-action@v0
        with:
          projectPath: crates/baton-gui
          args: --bundles ${{ matrix.bundle }}
      - name: Rename artifact
        shell: bash
        run: |
          mkdir -p dist
          if [ "${{ runner.os }}" = "macOS" ]; then
            src=$(find crates/baton-gui/target/release/bundle/dmg -name '*.dmg' | head -n 1)
          else
            src=$(find crates/baton-gui/target/release/bundle/nsis -name '*.exe' | head -n 1)
          fi
          cp "$src" "dist/${{ matrix.artifact }}"
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.artifact }}
          path: dist/${{ matrix.artifact }}
          retention-days: 7
```

Tauri 2 的 bundle 目录也可能在仓库根 `target/release/bundle/`（workspace）。`Rename artifact` 写成先找 `crates/baton-gui/target`，没有再找仓库根 `target`：

```bash
src=$(find crates/baton-gui/target/release/bundle target/release/bundle -name '*.dmg' -o -name '*setup.exe' -o -name 'DeskBaton_*_x64-setup.exe' 2>/dev/null | head -n 1)
```

按 `matrix.bundle` 分别 find，避免把错误文件拷走。

删 `.github/workflows/windows-build.yml`。

`TODO.md` 安装器项：把非 🔧 的五条打勾，留下「🔧 安装器设计验收清单第 1–5 项」。

- [ ] **Step 4: 跑测试**

Run: `cargo test -p baton-gui pack_workflow_builds_named_installers`

Expected: PASS

Run: `cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/pack.yml .github/workflows/windows-build.yml TODO.md crates/baton-gui/src/bundle_config.rs
git commit -m "$(cat <<'EOF'
ci: 构建 Mac/Windows 安装包

分发改为 dmg 与 NSIS，不再上传裸 exe。
EOF
)"
```

---

## Self-review

1. Spec coverage: 第 3 节 bundle / 文件名 → Task 1、6；第 4.1 登录自启 → Task 2、3；第 4.2 助手 → Task 4、5；第 4.3 NSIS → Task 2；第 5 卸载 Run 键 → Task 2；第 6 CI → Task 6；第 7 未签名 → 不实现（规格只要求文档，已在设计里）；第 8 代码组织 → 各 Task 路径一致。
2. 无 TBD。Windows `externalBin` 的处理已选定为 CI 临时清空。
3. FFI 名在 Task 3 定义，Task 5 使用相同四个符号。`allowed_argv` / `privileges_for` / `exe_is_bundled` 前后一致。
