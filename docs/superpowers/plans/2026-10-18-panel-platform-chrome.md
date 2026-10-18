# 面板分平台外壳与主题 token Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Windows 面板改成贴任务栏的右对齐浮层（先量高再显示、整块上滑）；macOS 保持菜单栏气泡；两边共用一份 `theme.css` token。

**Architecture:** 摆放纯函数加 `PanelAlign`（`CenterOnTray` / `WorkAreaEnd`），`main.rs` 按 `cfg(target_os)` 选对齐和间距。Windows 点托盘只发 `panel-will-show`，`fit_panel` 量完再 `show` 并发 `panel-shown`。颜色和外壳尺寸全部进 `theme.css`，`html.platform-windows` 只覆盖外壳变量。

**Tech Stack:** Rust 2024、`baton-gui`（Tauri 2）、纯 HTML/CSS/JS。不新增 crate、不写 toml。

## Global Constraints

- 不改交棒、配对、剪贴板、`PROTOCOL_VERSION`、面板分页和卡片内容。
- 不做主题选择器、named theme、浅色模式、从 `state.toml` / `config.toml` 注入颜色。
- Linux 继续排除 `baton-gui`；`cfg(not(target_os = "macos"))` 走 Windows 外壳（本 crate 不在 Linux 分发）。
- 用户文案中文；`unsafe` 只用于 FFI。
- 组件禁止再写主题色或外壳尺寸字面量（`#ff6a1a`、`18px` 外壳圆角等）。
- 提交前：`cargo fmt --all`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
- 提交信息：Conventional Commits 前缀 + 中文。仓库本地身份 `Rossettaylm <2441197035@qq.com>`，不改全局 git。
- 规格：`docs/superpowers/specs/2026-10-18-panel-platform-chrome-design.md`。

---

## File Structure

- Modify: `crates/baton-gui/src/placement.rs` — `PanelAlign`、`panel_origin` 第五参、`work_area_end_tray`
- Modify: `crates/baton-gui/src/main.rs` — 平台 gap / align、缺托盘矩形时的 Windows 回退、打开时序
- Modify: `crates/baton-gui/src/commands.rs` — `fit_panel` 在 Windows 上量完再 show；`host_platform`
- Create: `crates/baton-gui/ui/theme.css` — `:root` 与 `html.platform-windows`
- Modify: `crates/baton-gui/ui/style.css` — 首行 `@import`，组件只引用 `var(--…)`
- Modify: `crates/baton-gui/ui/main.js` — 加 `platform-*`；拆 `panel-will-show` / `panel-shown`
- Already written: spec、`TODO.md` 条目、面板旧设计冲突指针

---

### Task 1: `PanelAlign` 与摆放单测

**Files:**
- Modify: `crates/baton-gui/src/placement.rs`

**Interfaces:**
- Consumes: 现有 `Bounds`、`panel_origin(tray, panel, screen, gap)`
- Produces:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAlign {
    CenterOnTray,
    WorkAreaEnd,
}

pub fn panel_origin(
    tray: Bounds,
    panel: (f64, f64),
    screen: Bounds,
    gap: f64,
    align: PanelAlign,
) -> (f64, f64);

/// Windows 读不到托盘矩形时，用屏幕右下一块 36×24 充当托盘。
pub fn work_area_end_tray(screen: Bounds) -> Bounds;
```

- [ ] **Step 1: 改签名并先让旧测试带上 `CenterOnTray`**

把 `panel_origin` 的文档和签名改成五参数。函数体先原样只加 `let _ = align;`，或 `match align { CenterOnTray => { 旧逻辑 } WorkAreaEnd => { 旧逻辑 } }`，好让下一步写失败测试。

旧测试每一处调用补第五参 `PanelAlign::CenterOnTray`，期望值不动：

```rust
panel_origin(tray(900.0, 0.0), (360.0, 440.0), SCREEN, 6.0, PanelAlign::CenterOnTray)
// (738.0, 30.0)

panel_origin(tray(1870.0, 0.0), (360.0, 440.0), SCREEN, 6.0, PanelAlign::CenterOnTray)
// x == 1920.0 - 360.0 - 6.0

panel_origin(tray(1700.0, 1048.0), (360.0, 440.0), SCREEN, 6.0, PanelAlign::CenterOnTray)
// (1538.0, 602.0)

panel_origin(tray(1925.0, -515.0), (360.0, 440.0), screen, 6.0, PanelAlign::CenterOnTray)
// (1926.0, -485.0)
```

- [ ] **Step 2: 写失败的 Windows 对齐测试**

加在 `placement.rs` 的 `tests` 里：

```rust
#[test]
fn work_area_end_pins_bottom_right() {
    assert_eq!(
        panel_origin(
            tray(1700.0, 1048.0),
            (360.0, 440.0),
            SCREEN,
            12.0,
            PanelAlign::WorkAreaEnd,
        ),
        (1548.0, 596.0)
    );
}

#[test]
fn work_area_end_clamps_wide_panel() {
    let (x, _) = panel_origin(
        tray(1700.0, 1048.0),
        (2000.0, 440.0),
        SCREEN,
        12.0,
        PanelAlign::WorkAreaEnd,
    );
    assert_eq!(x, 12.0);
}

#[test]
fn work_area_end_clamps_y_to_screen() {
    let (_, y) = panel_origin(
        tray(1700.0, 100.0),
        (360.0, 440.0),
        SCREEN,
        12.0,
        PanelAlign::WorkAreaEnd,
    );
    assert_eq!(y, 0.0);
}

#[test]
fn work_area_end_tray_sits_at_bottom_right() {
    assert_eq!(
        work_area_end_tray(SCREEN),
        Bounds {
            x: 1884.0,
            y: 1056.0,
            width: 36.0,
            height: 24.0,
        }
    );
}
```

- [ ] **Step 3: 跑测试，确认新用例失败**

Run: `cargo test -p baton-gui --lib placement::tests::work_area_end_pins_bottom_right -- --exact`

Expected: FAIL（`WorkAreaEnd` 仍走居中，得到 `(1538, 596)` 或旧的 gap-6 上方坐标，不是 `(1548, 596)`）

- [ ] **Step 4: 按规格实现**

```rust
pub fn panel_origin(
    tray: Bounds,
    panel: (f64, f64),
    screen: Bounds,
    gap: f64,
    align: PanelAlign,
) -> (f64, f64) {
    let (width, height) = panel;
    let x = match align {
        PanelAlign::CenterOnTray => (tray.x + tray.width / 2.0 - width / 2.0)
            .min(screen.right() - width - gap)
            .max(screen.x + gap),
        PanelAlign::WorkAreaEnd => (screen.right() - width - gap).max(screen.x + gap),
    };
    let y = match align {
        PanelAlign::CenterOnTray => {
            let below = tray.bottom() + gap;
            if below + height <= screen.bottom() {
                below
            } else {
                (tray.y - height - gap).max(screen.y)
            }
        }
        PanelAlign::WorkAreaEnd => (tray.y - height - gap).max(screen.y),
    };
    (x, y)
}

pub fn work_area_end_tray(screen: Bounds) -> Bounds {
    Bounds {
        x: screen.x + screen.width - 36.0,
        y: screen.y + screen.height - 24.0,
        width: 36.0,
        height: 24.0,
    }
}
```

- [ ] **Step 5: 跑测试确认全过**

Run: `cargo test -p baton-gui --lib placement`

Expected: PASS，含全部旧用例和新用例。

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-10-18-panel-platform-chrome-design.md \
        docs/superpowers/specs/2026-10-16-display-panel-design.md \
        docs/superpowers/plans/2026-10-18-panel-platform-chrome.md \
        TODO.md \
        crates/baton-gui/src/placement.rs
git commit -m "$(cat <<'EOF'
feat: 面板摆放支持工作区右对齐

Windows 浮层不再水平居中托盘图标；设计文档与摆放单测一起落地。
EOF
)"
```

若规格文件已在更早提交里，只 add `placement.rs`，信息改为 `feat: 面板摆放支持工作区右对齐`。

---

### Task 2: `main.rs` 按平台选对齐，托盘缺失时 Windows 退到右下

**Files:**
- Modify: `crates/baton-gui/src/main.rs`（`GAP`、`try_place`、`place_panel`）

**Interfaces:**
- Consumes: `PanelAlign`、`panel_origin(..., align)`、`work_area_end_tray`
- Produces: macOS `GAP=6` + `CenterOnTray`；非 macOS `GAP=12` + `WorkAreaEnd`。`place_panel` 在没有托盘矩形时，Windows 用主屏（或第一块屏）+ `work_area_end_tray`，macOS 仍直接返回。

- [ ] **Step 1: 换常量**

`main.rs` 里现在的 `const GAP: f64 = 6.0;` 改成：

```rust
#[cfg(target_os = "macos")]
const GAP: f64 = 6.0;
#[cfg(not(target_os = "macos"))]
const GAP: f64 = 12.0;

#[cfg(target_os = "macos")]
const PANEL_ALIGN: placement::PanelAlign = placement::PanelAlign::CenterOnTray;
#[cfg(not(target_os = "macos"))]
const PANEL_ALIGN: placement::PanelAlign = placement::PanelAlign::WorkAreaEnd;
```

- [ ] **Step 2: `try_place` 接 `Option<Rect>`**

`place_panel` 在 `live.or(stored)` 为 `None` 时不要直接 return：

```rust
pub(crate) fn place_panel(window: &WebviewWindow) {
    let live = window
        .app_handle()
        .tray_by_id("main")
        .and_then(|tray| tray.rect().ok().flatten());
    let stored = *window
        .state::<TrayRect>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Err(e) = try_place(window, live.or(stored)) {
        log::warn!("摆放面板失败：{e}");
    }
}
```

`try_place(window, rect: Option<Rect>)`：

1. 取 `available_monitors`，空则 `return Ok(())`。
2. 有 `rect` 时保持现在的「按缩放换回逻辑坐标、找所在屏」逻辑。
3. 无 `rect`：macOS `return Ok(())`。非 macOS 取 `primary_monitor` 否则第一块，`logical_bounds` 得到 `screen`，`tray = work_area_end_tray(screen)`。
4. 调用改为：

```rust
let (x, y) = placement::panel_origin(
    tray,
    (panel.width, panel.height),
    screen,
    GAP,
    PANEL_ALIGN,
);
```

坐标写入（macOS 逻辑 / 其它物理）保持原样。

- [ ] **Step 3: 编译该 crate**

Run: `cargo test -p baton-gui --lib`

Expected: PASS（摆放测试仍过；`main.rs` 能编过）。

- [ ] **Step 4: Commit**

```bash
git add crates/baton-gui/src/main.rs
git commit -m "$(cat <<'EOF'
feat: Windows 面板贴工作区右下

按平台选用 CenterOnTray 或 WorkAreaEnd；托盘矩形缺失时退到该屏右下。
EOF
)"
```

---

### Task 3: Windows 先 `fit_panel` 再显示

**Files:**
- Modify: `crates/baton-gui/src/main.rs`（`toggle_panel`、`show_panel`）
- Modify: `crates/baton-gui/src/commands.rs`（`fit_panel`、`host_platform`；`invoke_handler` 在 `main.rs`）
- Modify: `crates/baton-gui/ui/main.js`（事件拆分、`fit` 失败重试）

**Interfaces:**
- Consumes: 现有 `panel-shown`、`fit()`、`place_panel`
- Produces:
  - `host_platform() -> &'static str`：`"macos"` 或 `"windows"`
  - 事件 `panel-will-show`（仅 Windows 打开时、尚未 `show`）
  - `fit_panel`：`set_size` → `place_panel` → 若窗口不可见则 `show` + `focus` + 发 `panel-shown`
  - macOS `toggle_panel` / `show_panel` 仍先 `show` 再发 `panel-shown`，不发 `panel-will-show`

- [ ] **Step 1: 加 `host_platform` 并注册**

`commands.rs`：

```rust
#[tauri::command]
pub fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "windows"
    }
}
```

`main.rs` 的 `invoke_handler` 列表加上 `commands::host_platform`。

- [ ] **Step 2: 抽出打开函数，按平台分支**

`main.rs` 把 `toggle_panel` / `show_panel` 里「place + show + emit」换成：

```rust
fn reveal_or_request(app: &AppHandle, window: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    {
        place_panel(window);
        if let Err(e) = window.show().and_then(|()| window.set_focus()) {
            log::warn!("显示面板失败：{e}");
        }
        let _ = app.emit_to(PANEL, "panel-shown", ());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app.emit_to(PANEL, "panel-will-show", ());
    }
}
```

`toggle_panel` 在通过「已可见则隐藏 / 300ms grace」之后调用 `reveal_or_request`。`show_panel` 在「已可见则 return」之后调用同一函数。

- [ ] **Step 3: `fit_panel` 在未显示时负责 `show`**

```rust
#[tauri::command]
pub fn fit_panel(app: AppHandle, height: f64) -> std::result::Result<(), String> {
    let window = app.get_webview_window(PANEL).ok_or("面板窗口不存在")?;
    window
        .set_size(LogicalSize::new(PANEL_WIDTH, height.ceil()))
        .map_err(|e| e.to_string())?;
    place_panel(&window);
    if !window.is_visible().unwrap_or(false) {
        if let Err(e) = window.show().and_then(|()| window.set_focus()) {
            return Err(e.to_string());
        }
        let _ = app.emit_to(PANEL, "panel-shown", ());
    }
    Ok(())
}
```

macOS 打开时窗口已经 visible，这个分支不会再发一次 `panel-shown`。Windows 打开时窗口仍隐藏，这里是唯一 `show` 入口。

- [ ] **Step 4: 前端拆事件**

`main.js` 把现有 `panel-shown` 监听改成：

```js
function applyPlatform(p) {
  document.documentElement.classList.add(`platform-${p}`);
}

invoke("host_platform").then(applyPlatform).catch(() => {});

function requestFit() {
  cancelAnimationFrame(fitFrame);
  fitFrame = requestAnimationFrame(() => {
    const panel = $("panel").getBoundingClientRect();
    const menu = $("menu");
    const bottom = menu.hidden ? panel.bottom : Math.max(panel.bottom, menu.getBoundingClientRect().bottom);
    const height = Math.ceil(bottom) || 480;
    invoke("fit_panel", { height }).catch(() => {
      invoke("fit_panel", { height: 480 }).catch(() => {});
    });
  });
}

function fit() {
  requestFit();
}

tauri.event.listen("panel-will-show", () => {
  closeMenu();
  requestFit();
  if (state.pending == null) refresh();
  refreshDevices();
  refreshPrefs();
});

tauri.event.listen("panel-shown", () => {
  closeMenu();
  reveal();
  if (!document.documentElement.classList.contains("platform-windows")) {
    if (state.pending == null) refresh();
    refreshDevices();
    refreshPrefs();
  }
});
```

macOS 没有 `panel-will-show`，`panel-shown` 仍负责 refresh + reveal。Windows 刷新只在 `panel-will-show`；`panel-shown` 只播 `reveal`。

- [ ] **Step 5: 编译与单测**

Run: `cargo test -p baton-gui --lib`

Expected: PASS。

- [ ] **Step 6: Commit**

```bash
git add crates/baton-gui/src/main.rs crates/baton-gui/src/commands.rs crates/baton-gui/ui/main.js
git commit -m "$(cat <<'EOF'
feat: Windows 面板量高后再显示

避免 tauri 初始 480 高度先闪一帧；macOS 仍先显示再 fit。
EOF
)"
```

---

### Task 4: `theme.css` 共享 token，清掉组件字面量

**Files:**
- Create: `crates/baton-gui/ui/theme.css`
- Modify: `crates/baton-gui/ui/style.css`（删 `:root` 色块，首行 `@import url("theme.css");`，所有硬编码色/外壳圆角改 `var(--…)`）

**Interfaces:**
- Consumes: 规格 §5.1 / §5.2
- Produces: `theme.css` 为唯一主题真相；`style.css` 不再出现 `#` 颜色或 `rgba(255, 106, 26` / `rgba(61, 220, 132` / `rgba(255, 90, 78`

- [ ] **Step 1: 写 `theme.css`**

完整文件：

```css
:root {
  --bg: #121315;
  --card: #1a1b1e;
  --card-inset: #141517;
  --menu: #1e1f23;
  --line: rgba(255, 255, 255, 0.07);
  --line-strong: rgba(255, 255, 255, 0.12);
  --line-hover: rgba(255, 255, 255, 0.16);
  --line-focus: rgba(255, 255, 255, 0.22);
  --ink: #eceae6;
  --mute: #8a8781;
  --signal: #ff6a1a;
  --signal-hot: #ff7a2e;
  --signal-deep: #e2520c;
  --signal-dim: rgba(255, 106, 26, 0.16);
  --signal-glow: rgba(255, 106, 26, 0.28);
  --signal-line: rgba(255, 106, 26, 0.55);
  --signal-fill: rgba(255, 106, 26, 0.25);
  --signal-pending: rgba(255, 106, 26, 0.4);
  --ok: #3ddc84;
  --ok-dim: #24352c;
  --ok-line: rgba(61, 220, 132, 0.5);
  --ok-line-dim: rgba(61, 220, 132, 0.4);
  --ok-wash: rgba(61, 220, 132, 0.1);
  --ok-glow: rgba(61, 220, 132, 0.6);
  --err: #ff5a4e;
  --err-ink: #e9a19b;
  --err-line: rgba(255, 90, 78, 0.4);
  --bezel: #2a2b2f;
  --screen: #0b0b0c;
  --led-off: #3a3b40;
  --led-port: #2c2d31;
  --logo-ink: #ffffff;
  --hover: rgba(255, 255, 255, 0.04);
  --hover-strong: rgba(255, 255, 255, 0.06);
  --scanline: rgba(255, 255, 255, 0.05);
  --menu-shadow: 0 16px 32px -12px rgba(0, 0, 0, 0.8);
  --radius-card: 12px;
  --radius-port: 9px;
  --panel-width: 360px;
  --radius-shell: 18px;
  --edge-gap: 6px;
  --reveal-shift: 8px;
  --reveal-ms: 500ms;
  --mono: "Martian Mono", -apple-system, "PingFang SC", "Microsoft YaHei", monospace;
  --sans: -apple-system, "PingFang SC", "Microsoft YaHei", sans-serif;
}

html.platform-windows {
  --radius-shell: 12px;
  --edge-gap: 12px;
  --reveal-shift: 12px;
  --reveal-ms: 220ms;
}
```

规格 §5.1 表里的名字必须都在。`--line-hover` 等是为了清掉组件里剩余字面量，也只写在这个文件。

- [ ] **Step 2: `style.css` 导入并替换**

文件第一行：

```css
@import url("theme.css");
```

删掉原来的 `:root { --bg: … --sans: … }`（字体栈已在 `theme.css`）。

按表替换（只改值，不改选择器结构）：

| 现在 | 改为 |
|---|---|
| `.panel` `width: 360px` | `width: var(--panel-width)` |
| `.panel` `border-radius: 18px` | `border-radius: var(--radius-shell)` |
| `.panel` `box-shadow: inset 0 1px 0 rgba(255,255,255,.04)` | `inset 0 1px 0 var(--hover)` |
| `@keyframes rise` `translateY(8px)` | `translateY(var(--reveal-shift))` |
| `.panel.reveal > *` `0.5s` | `var(--reveal-ms)` |
| `.logo` 渐变 `#ff7a2e, #e2520c` | `var(--signal-hot), var(--signal-deep)` |
| `.logo svg` `stroke: #fff` | `stroke: var(--logo-ink)` |
| `.more:hover` 背景 `rgba(255,255,255,.04)` | `var(--hover)` |
| `.menu` 背景 `#1e1f23` | `var(--menu)` |
| `.menu` 边框 `rgba(255,255,255,.1)` | `var(--line-strong)` |
| `.menu` `border-radius: 12px` | `var(--radius-card)` |
| `.menu` `box-shadow: 0 16px…` | `var(--menu-shadow)` |
| `.menu button:hover` `rgba(255,255,255,.06)` | `var(--hover-strong)` |
| `.screen` / `.card` `border-radius: 12px` | `var(--radius-card)` |
| `.bezel` 边 `#2a2b2f` 底 `#0b0b0c` | `var(--bezel)` / `var(--screen)` |
| `.bezel::before` 辉光 `rgba(255,106,26,.28)` | `var(--signal-glow)` |
| `.bezel::after` `rgba(255,255,255,.05)` | `var(--scanline)` |
| `.stand` `#2a2b2f` | `var(--bezel)` |
| `.now.away` `#3a3b40` | `var(--led-off)` |
| `.port` 底 `#141517` 圆角 `9px` | `var(--card-inset)` / `var(--radius-port)` |
| `.port:hover` 边 `rgba(255,255,255,.16)` | `var(--line-hover)` |
| `.port .led` `#2c2d31` | `var(--led-port)` |
| `.port.on` 边 `rgba(255,106,26,.55)` | `var(--signal-line)` |
| `.port.on svg` `fill: rgba(255,106,26,.25)` | `var(--signal-fill)` |
| `.port.pending` 边 `rgba(255,106,26,.4)` | `var(--signal-pending)` |
| `.btn` 边 `rgba(255,255,255,.12)` | `var(--line-strong)` |
| `.btn:hover` 底/边 | `var(--hover)` / `var(--line-hover)` |
| `.btn.primary` 边/悬停 | `var(--ok-line)` / `var(--ok-wash)` |
| `.btn.ghost.danger` 边 | `var(--err-line)` |
| `.pair-request` 边/底 | `var(--line-strong)` / `var(--card-inset)` |
| `.device .dot` / `.status::before` `#3a3b40` | `var(--led-off)` |
| `.device.online .dot` 光晕 | `var(--ok-glow)` |
| `.status[data-tone="err"]` `#e9a19b` | `var(--err-ink)` |
| `.pref` 输入底 `#141517` | `var(--card-inset)` |
| `.pref` focus 边 | `var(--line-focus)` |
| checkbox 关 `#2a2b2f` | `var(--bezel)` |
| checkbox 开 `#24352c` 与 `rgba(61,220,132,.4)` | `var(--ok-dim)` / `var(--ok-line-dim)` |

噪点 SVG 数据 URI 保留（不是主题色）。

- [ ] **Step 3: 用 ripgrep 确认 `style.css` 没有主题字面量**

Run:

```bash
rg -n '#[0-9a-fA-F]{3,8}|rgba\(\s*255\s*,\s*106\s*,\s*26|rgba\(\s*61\s*,\s*220\s*,\s*132|rgba\(\s*255\s*,\s*90\s*,\s*78' crates/baton-gui/ui/style.css
```

Expected: 无匹配。若只剩下噪点 SVG 里的无关内容，把选择器缩到颜色声明即可。

- [ ] **Step 4: Commit**

```bash
git add crates/baton-gui/ui/theme.css crates/baton-gui/ui/style.css
git commit -m "$(cat <<'EOF'
feat: 面板主题收成 theme.css token

换强调色只改一份变量；组件不再写信号橙和外壳圆角字面量。
EOF
)"
```

---

### Task 5: Windows 整块上滑，取消卡片逐张浮入

**Files:**
- Modify: `crates/baton-gui/ui/style.css`（动画）
- Modify: `crates/baton-gui/ui/main.js`（Task 3 已加 `platform-*`；本任务确认 class 在 `reveal()` 之前就位）

**Interfaces:**
- Consumes: `--reveal-shift`、`--reveal-ms`、`html.platform-windows`
- Produces: macOS 仍是子元素 `rise`；Windows 是 `.panel.reveal` 自身 `slide-up`，子元素无 animation

- [ ] **Step 1: 改动画规则**

`style.css` 里现有 `@keyframes rise` 与 `.panel.reveal > *` 改成：

```css
@keyframes rise {
  from { opacity: 0; transform: translateY(var(--reveal-shift)); }
  to { opacity: 1; transform: none; }
}

@keyframes slide-up {
  from { opacity: 0; transform: translateY(var(--reveal-shift)); }
  to { opacity: 1; transform: none; }
}

.panel.reveal > * { animation: rise var(--reveal-ms) cubic-bezier(0.2, 0.7, 0.2, 1) both; }
.panel.reveal > :nth-child(2) { animation-delay: 0.06s; }
.panel.reveal > :nth-child(3) { animation-delay: 0.12s; }
.panel.reveal > :nth-child(4) { animation-delay: 0.18s; }

html.platform-windows .panel.reveal {
  transform-origin: bottom center;
  animation: slide-up var(--reveal-ms) cubic-bezier(0.2, 0.7, 0.2, 1) both;
}

html.platform-windows .panel.reveal > * {
  animation: none;
}
```

- [ ] **Step 2: 确认 `host_platform` 在首次 `reveal` 前加上 class**

`main.js` 顶部的 `invoke("host_platform").then(applyPlatform)` 必须在文件加载时就发起，不能放进 `panel-shown` 之后。Windows 窗口在 `fit_panel` 里才 `show`，class 有时间就位。若 `invoke` 尚未返回，`applyPlatform` 仍会在 reveal 前或动画第一帧内加上 class——可接受。不要用 User-Agent。

- [ ] **Step 3: Commit**

```bash
git add crates/baton-gui/ui/style.css crates/baton-gui/ui/main.js
git commit -m "$(cat <<'EOF'
feat: Windows 面板从任务栏整块上滑

macOS 保持卡片依次上浮；Windows 取消子元素 translateY。
EOF
)"
```

---

### Task 6: 提交前检查并勾实现项

**Files:**
- Modify: `TODO.md`（显示器面板下「Windows 任务栏浮层 + 共享主题 token」打勾）

**Interfaces:**
- Consumes: 本计划 Task 1–5
- Produces: 工作区检查通过；实现项打勾。规格 §9 手动第 2–3 项仍是 🔧，不打勾。

- [ ] **Step 1: 跑提交前检查**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: 全部通过。Linux CI 仍排除 `baton-gui`，本机 macOS 会编过 `baton-gui`。

- [ ] **Step 2: 勾 TODO**

```markdown
- [x] Windows 任务栏浮层 + 共享主题 token（[设计](docs/superpowers/specs/2026-10-18-panel-platform-chrome-design.md)）
```

在同一条下面补 🔧，不要提前勾：

```markdown
  - [ ] 🔧 Windows：面板贴任务栏上方、距右缘 12px，打开无 480 高闪帧
  - [ ] 🔧 Windows：切分页或展开「⋯」时底边不动、只往上长
```

- [ ] **Step 3: Commit**

```bash
git add TODO.md
git commit -m "$(cat <<'EOF'
docs: 勾选面板分平台外壳实现项

Windows 贴边与上滑动效留给用户实测后再勾。
EOF
)"
```

---

## Spec coverage

| 规格 | 任务 |
|---|---|
| §3 水平 / 垂直 / `PanelAlign` | Task 1–2 |
| §3 缺托盘矩形 → 右下 | Task 1 `work_area_end_tray`、Task 2 |
| §3 GAP 与 `--edge-gap` 同值、Rust 不读 CSS | Task 2 常量、Task 4 token |
| §4 / §5.2 圆角与动效 | Task 4–5 |
| §5.1 共享 token、禁止字面量 | Task 4 |
| §5 `html.platform-*`、不靠 UA | Task 3 `host_platform`、Task 5 |
| §6 Windows 先 fit 再 show、`panel-will-show` / `panel-shown` | Task 3 |
| §6 macOS 时序不变 | Task 3 `#[cfg(target_os = "macos")]` |
| §9 摆放单测 | Task 1 |
| §9 手动 🔧 | Task 6 列入 TODO，不打勾 |
| 不做主题选择器 / toml / 改交棒 | 全任务均不触及 |
