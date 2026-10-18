# 面板分平台外壳与主题 token

日期：2026-10-18　状态：已确认

主设计见 `2026-10-16-deskbaton-design.md`，面板见 `2026-10-16-display-panel-design.md`。冲突时以本文为准。覆盖面板设计第 7 节「窗口形态 / 摆放 / 动效」，以及第 7.1 节色彩表。

## 1. 目标

macOS 继续从菜单栏图标往下弹气泡。Windows 改成贴任务栏的系统浮层，不再水平对齐托盘图标、也不再从右下角往上长。

两边同一套主题 token：换强调色或底色只改一份变量。平台只覆盖外壳（圆角、边距、打开动画）。

## 2. 范围

**做**

- Windows：工作区右对齐、钉在任务栏上方、先量高再显示、整块上滑
- 主题色、圆角、间距、时长全部收成 CSS 变量；组件不再写主题色或外壳尺寸的字面量
- `html.platform-macos` / `html.platform-windows` 只覆盖外壳变量
- `panel_origin` 按平台选对齐方式，单测覆盖 Windows 右对齐与底边钉死

**不做**

- 改面板分页、卡片内容、交棒流程
- 主题选择器、多套 named theme、从 `state.toml` / `config.toml` 注入颜色
- 浅色模式、跟随系统外观
- Linux 面板（本来就不分发）
- 托盘短菜单、独立可拖窗口

以后加 `[data-theme=…]` 或从偏好注入，只改 token 文件，不改组件。

## 3. 交互与摆放

打开、关闭、右键「退出」、失焦隐藏、点图标后 300ms 内不重开：两边与现在相同。

| | macOS | Windows |
|---|---|---|
| 水平 | 图标居中，再夹进屏幕，边距 `--edge-gap`（6px） | 工作区右对齐，`x = screen.right - width - gap`，边距 12px |
| 垂直 | 图标下方放得下就下方，否则上方 | 永远在托盘上方：`y = tray.y - height - gap`，下限 `screen.y` |
| 显示 | 先 `show` 再 `fit_panel`（现状） | **先 `fit_panel`，再 `show` + `focus`** |
| 高度变化 | 重新摆放 | 底边钉在 `tray.y - gap`，只往上长 |

多屏：仍用托盘图标所在那块屏。托盘矩形读不到：Windows 退到该屏（或主屏）右下，按同样的右对齐和底边钉死，不居中。

`placement.rs` 的 `GAP` 与 CSS `--edge-gap` 数值相同，但 Rust 不读 CSS：macOS `6.0`，Windows `12.0`。

```rust
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
```

`CenterOnTray` 保持现有水平算法。`WorkAreaEnd` 取 `screen.right - width - gap`，再与 `screen.x + gap` 取较大值，避免比屏幕还宽时出界。垂直算法按上表，不复用「下方优先」。

Windows 11 任务栏只在底部。顶栏或不在底部的任务栏不单独处理。

## 4. 视觉与动效

内容、字体、始终深色、橙色只用于当前信号：与面板设计 7.1 相同。

| | macOS | Windows |
|---|---|---|
| `--radius-shell` | 18px | 12px |
| 打开动画 | 各卡片上浮 `--reveal-shift`（8px），时长 500ms，间隔 60ms | **整块**面板从下上滑 12px + 淡入，`transform-origin: bottom`，220ms；取消卡片逐张 `translateY` |
| 卡片 / 接口圆角 | `--radius-card` 12px、`--radius-port` 9px，两边相同 | 同左 |

## 5. 主题 token

唯一真相：`crates/baton-gui/ui/theme.css`，由 `style.css` 在最前面导入。`:root` 写共享默认（含 macOS 外壳）。`html.platform-windows` 只覆盖 §5.2。

换主题色 = 改 `:root` 里 §5.1 的值。禁止在 `style.css` 或 JS 里再写 `#ff6a1a`、`18px` 这类主题字面量。装饰性透明白（`rgba(255,255,255,.04)` 悬停）也收成 token。

启动时 Rust 给面板窗口执行一次脚本，给 `html` 加上 `platform-macos` 或 `platform-windows`。不靠 User-Agent。

### 5.1 共享（改主题只动这里）

现有 9 个色保留原意；把组件里散落的硬编码收进来。默认值等于今天画面。

| 变量 | 默认 | 用途 |
|---|---|---|
| `--bg` | `#121315` | 面板底 |
| `--card` | `#1A1B1E` | 卡片底 |
| `--card-inset` | `#141517` | 输入框、接口格、配对条等凹槽 |
| `--menu` | `#1E1F23` | 「⋯」菜单底 |
| `--line` | `rgba(255,255,255,.07)` | 默认描边、分隔 |
| `--line-strong` | `rgba(255,255,255,.12)` | 按钮、配对条描边 |
| `--ink` | `#ECEAE6` | 主文字 |
| `--mute` | `#8A8781` | 次要文字 |
| `--signal` | `#FF6A1A` | 当前信号 |
| `--signal-hot` | `#FF7A2E` | logo 渐变亮端 |
| `--signal-deep` | `#E2520C` | logo 渐变暗端 |
| `--signal-dim` | `rgba(255,106,26,.16)` | 选中底 |
| `--signal-glow` | `rgba(255,106,26,.28)` | 预览屏辉光 |
| `--signal-line` | `rgba(255,106,26,.55)` | 选中描边 |
| `--ok` | `#3DDC84` | 正常、在线 |
| `--ok-dim` | `#24352C` | 打开的开关底 |
| `--err` | `#FF5A4E` | 错误 |
| `--err-ink` | `#E9A19B` | 错误状态文字 |
| `--bezel` | `#2A2B2F` | 屏框、底座、关着的开关 |
| `--screen` | `#0B0B0C` | 预览屏内 |
| `--led-off` | `#3A3B40` | 熄灭的状态点 |
| `--led-port` | `#2C2D31` | 接口指示灯默认 |
| `--logo-ink` | `#FFFFFF` | logo 描边 |
| `--hover` | `rgba(255,255,255,.04)` | 悬停底 |
| `--radius-card` | `12px` | 卡片、菜单 |
| `--radius-port` | `9px` | 接口 / 电脑格 |
| `--panel-width` | `360px` | 与 Rust `PANEL_WIDTH` 同值 |

`--signal-dim` / `--signal-glow` / `--signal-line` 是 `--signal` 的固定透明变体，改强调色时三套一起改。这一轮不在运行时从 `--signal` 推算。

面板宽仍由 Rust `PANEL_WIDTH` 定窗口；CSS `--panel-width` 只给 `.panel` 用，两边都写 `360`。

### 5.2 外壳（平台覆盖）

| 变量 | macOS（`:root` 默认） | Windows |
|---|---|---|
| `--radius-shell` | `18px` | `12px` |
| `--edge-gap` | `6px` | `12px` |
| `--reveal-shift` | `8px` | `12px` |
| `--reveal-ms` | `500ms` | `220ms` |

`--edge-gap` 给 CSS 间距用；Rust 摆放用同名含义的常量，不解析这套 CSS。

## 6. 显示时序（Windows）

现在：`show` → 前端量高 → `fit_panel` 改高度并重贴。首帧按 `tauri.conf.json` 的 480 高出现，再往上长。

改为：

1. 点托盘：若面板已可见则隐藏（现状）。
2. 若面板隐藏：设置一次性待打开标记，发 `panel-will-show`，**不** `show`。
3. 前端刷新并 `fit()`。`fit_panel` 失败则用当前窗口高度再调用一次（没有当前高度则用 480），避免永远不出现。
4. `fit_panel`：`set_size` → `place_panel` → 仅在待打开标记存在且窗口尚未可见时消费标记、`show` + `focus`，再发 `panel-shown`。
5. 普通后台 `fit_panel` 只改尺寸和位置，不得显示隐藏窗口；隐藏面板时清除待打开标记，避免延迟的 `fit_panel` 再次弹出。
6. `panel-shown` 只播 `reveal`。已可见时的 `fit_panel` 不再发 `panel-shown`。

macOS 保持先 `show` 再 `fit`，避免改一条已经验收过的路径。

`tauri.conf.json` 初始高度仍可 480：Windows 上用户看不见这一帧。

## 7. 代码组织

```text
baton-gui/ui/theme.css     :root 与 html.platform-windows 的变量；style.css 首行导入
baton-gui/ui/style.css     组件只引用 var(--…)
baton-gui/src/placement.rs panel_origin 增加 PanelAlign；单测分平台
baton-gui/src/main.rs      启动时给 html 加 platform-*；Windows 摆放用 WorkAreaEnd 与 gap 12
baton-gui/src/commands.rs  fit_panel：Windows 仅消费待打开标记后量完再 show
```

不把主题放进 `baton-core`，不进 toml。

## 8. 既有文档

- 面板设计第 7 节「窗口形态 / 摆放」：Windows 改为本文 §3、§6。
- 面板设计第 7 节「动效」：Windows 改为本文 §4。
- 面板设计第 7.1 节色彩表：升级为本文 §5.1；外壳尺寸从 7.1「尺寸」里拆出，按 §5.2 分平台。
- 7.1「始终深色、橙色只用于当前信号」仍然有效。

## 9. 测试与验收

**单测（`placement.rs`）**

- `CenterOnTray`：现有用例保持原期望值（补上 `align` 参数即可）
- `WorkAreaEnd`：1920×1080、托盘 `(1700, 1048, 36×24)`、面板 360×440、gap 12 → `(1548, 596)`
- 面板比屏幕还宽时，`x = screen.x + gap`
- 上方空间不够时，`y = screen.y`

**CI**：现有 `fmt` / `clippy -D warnings` / `test`；Linux 继续排除 `baton-gui`。

**手动**

1. macOS：面板仍在菜单栏图标正下方弹出，圆角 18，卡片上浮；再点或点别处收起。
2. Windows：面板贴在主屏任务栏上方、距右缘 12px，圆角 12；打开是整块上滑，没有先出现一块 480 高再长高。
3. Windows：切换「显示器 / 设备 / 设置」或展开「⋯」时，底边不动、只往上长。
4. 把 `--signal` / `--signal-hot` / `--signal-deep` / `--signal-dim` / `--signal-glow` / `--signal-line` 改成另一色后重载，logo、高亮电脑、信号灯、预览辉光一起变，组件文件不用改。

第 2–3 项标 🔧，用户在 Windows 上确认后打勾。

## 10. 风险

| 风险 | 应对 |
|---|---|
| Windows 先 fit 再 show，首开若前端没量到高 | `fit_panel` 收到高度后再 show；超时或失败仍按上次高度 show，并记日志 |
| 托盘在溢出区，右对齐看起来不跟图标对 | 这是方案 A 的预期：跟系统音量/网络浮层一样贴工作区右下 |
| 漏收硬编码色，改主题色漏一块 | 验收第 4 项；实现时 `style.css` 里不应再出现 `#` 或 `rgba(255, 106, 26` |
| macOS 被连带改坏 | 摆放默认与测试保持 `CenterOnTray`；显示时序不改 |
