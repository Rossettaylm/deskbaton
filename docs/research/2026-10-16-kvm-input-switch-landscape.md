# 跨平台输入源流转控制：开源生态调研

> 调研日期：2026-10-16。Star 数、最近提交时间均取自 GitHub API（`api.github.com/repos/<owner>/<repo>`）当日数据。
> 标注「未验证」的内容没有找到一手来源，需要后续实测。

## 0. 结论速览

1. **需求是真的，但没有头部玩家。** 大型软件 KVM（Deskflow 2.97 万星、Lan Mouse 5.4k 星）都不管显示器输入源；
   而"软件 KVM + DDC 切输入源"的组合项目在 2026 年集中冒出了一批，但都很小（0–7 星），且多数只覆盖单平台或两台机器。
2. **最关键的技术事实**：很多显示器只接受「当前正在显示的那路输入」发来的 DDC 命令。
   所以"想切过去的那台机器"往往发不出命令，必须通过局域网把切换请求**委托**给当前占着屏幕的机器去发。
   这决定了产品必须是「每台机器都装 agent + 局域网协同」的架构。
3. **唤醒睡眠中的 Mac 是纯软件方案的短板。** Mac 上的 agent 睡着了收不到任何东西；
   Wake-on-LAN 只在有线以太网下勉强可用，Apple Silicon 和 macOS Tahoe 上都有失效报告。
   要做到可靠唤醒，需要一个可选的 USB HID 小硬件（RP2040 / ESP32-S3 级别）模拟一次按键。
4. **命名避开 "Flow"**：罗技在同一领域持有 Flow™ 商标（Logitech Flow 就是多电脑键鼠流转），
   Deskflow 也占了这个词。`SystemFlow` 作为产品名风险高，可保留为内部代号。

## 1. 软件 KVM（键鼠共享）

| 项目 | 语言 | 许可证 | Star | 最近提交 | 状态 / 要点 |
|---|---|---|---|---|---|
| [deskflow/deskflow](https://github.com/deskflow/deskflow) | C++ | GPL-2.0 | 29,740 | 2026-10-15 | Synergy 的官方上游；最新 v1.27.0（2026-10-01）。默认 TLS、支持 Wayland（需 libei 1.3+ / libportal 0.8+）、Win/mac/Linux/BSD。兼容 Synergy 1 协议 |
| [symless/synergy](https://github.com/symless/synergy) | C++ | GPL-2.0 | 314 | 2026-10-14 | Deskflow 的下游商业版，Synergy 3 与 Deskflow 尚不兼容 |
| [debauchee/barrier](https://github.com/debauchee/barrier) | C | 自定义(GPL 系) | 30,923 | 2024-06-22 | 已停止维护 |
| [input-leap/input-leap](https://github.com/input-leap/input-leap) | C++ | 自定义(GPL 系) | 8,320 | 2025-12-01 | **仓库已归档**；Deskflow README 称其为 "dead fork" |
| [feschber/lan-mouse](https://github.com/feschber/lan-mouse) | Rust | GPL-3.0 | 5,380 | 2026-09-26 | 点对点架构；全流量 DTLS 加密（webrtc-rs）；支持 Win/mac/Wayland（wlroots、libei、portal 多后端）；X11 只能做接收端 |
| [htrefil/rkvm](https://github.com/htrefil/rkvm) | Rust | MIT | 552 | 2026-09-26 | 仅 Linux，基于 evdev/uinput |
| [r-c-f/waynergy](https://github.com/r-c-f/waynergy) | C | MIT | 456 | 2026-09-13 | Wayland 下的 Synergy 客户端 |
| [microsoft/PowerToys](https://github.com/microsoft/PowerToys) Mouse Without Borders | C# | MIT | 139k（整个 PowerToys） | 2026-10-16 | 仅 Windows |

来源：各仓库 README；Deskflow 与 Synergy/Input Leap 关系见 [Deskflow README 相关项目段](https://github.com/deskflow/deskflow#readme)；Lan Mouse 加密与后端支持见 [Lan Mouse README](https://github.com/feschber/lan-mouse#readme)。

**对我们的意义**

- 键鼠这一层不该从零写：Windows UAC 安全桌面、macOS 辅助功能权限、Linux Wayland 的捕获与注入，这些坑 Deskflow 和 Lan Mouse 已经踩了多年。
- 许可证：Deskflow 是 GPL-2.0，Lan Mouse 是 GPL-3.0。直接链接或复用代码，产品就得以 GPL 开源；
  以独立进程方式调用 `deskflow-core`（Moniswitch、one-for-all-switch 都是这么做的）通常被视为"聚合"而非衍生作品，但这不是法律意见，商业化前要确认。
- rkvm 是 MIT，但只支持 Linux，参考价值有限。

## 2. DDC/CI 切换显示器输入源

### 2.1 底层库与工具

| 项目 | 平台 | 语言 / 许可证 | Star | 最近提交 | 说明 |
|---|---|---|---|---|---|
| [rockowitz/ddcutil](https://github.com/rockowitz/ddcutil) | Linux | C / GPL-2.0 | 1,417 | 2026-10-15 | Linux 事实标准；文档里沉淀了大量显示器怪癖 |
| [haimgel/display-switch](https://github.com/haimgel/display-switch) | Win/mac/Linux | Rust / MIT | 3,458 | 2026-09-01 | 最接近的先例，详见 2.2 |
| [waydabber/m1ddc](https://github.com/waydabber/m1ddc) | macOS Apple Silicon | ObjC / MIT | 747 | 2026-08-02 | CLI；支持 USB-C/DP Alt Mode 和"部分"内置 HDMI 口；**不支持 Intel Mac** |
| [MonitorControl/MonitorControl](https://github.com/MonitorControl/MonitorControl) | macOS | Swift / MIT | 34,486 | 2026-10-14 | 以亮度/音量为主 |
| [alin23/Lunar](https://github.com/alin23/Lunar) | macOS | Swift / MIT | 5,717 | 2026-10-11 | 自适应亮度，带 CLI |
| [waydabber/BetterDisplay](https://github.com/waydabber/BetterDisplay) | macOS | 闭源 | 34,077 | 2026-10-06 | 仓库只有发布物，支持所有 Mac 所有口 |
| [xanderfrangos/twinkle-tray](https://github.com/xanderfrangos/twinkle-tray) | Windows | JS / MIT | 9,186 | 2026-10-12 | 以亮度为主 |
| [emoacht/Monitorian](https://github.com/emoacht/Monitorian) | Windows | C# / MIT | 4,762 | 2026-10-15 | 以亮度为主 |
| [arcnmx/ddc-hi-rs](https://github.com/arcnmx/ddc-hi-rs) / [ddcset-rs](https://github.com/arcnmx/ddcset-rs) | Win/Linux(+mac 后端) | Rust / MIT | 28 / 40 | 2023-06-06 | Rust 跨平台 DDC 抽象，**已 3 年未更新** |

m1ddc 的平台限制见 [m1ddc README](https://github.com/waydabber/m1ddc#readme)；其中提到配套的 [AppleSiliconDDC](https://github.com/waydabber/AppleSiliconDDC) 开源版缺少 M1 HDMI 支持。

### 2.2 display-switch 架构拆解（最接近的先例）

- 机制：监听某个 USB 设备（如 USB 切换器下挂的设备）的插拔事件，触发 DDC 切输入源。"30 美元 USB 切换器 + 软件 = 完整 KVM"。
- 配置：一个 ini 文件，`usb_device = "VID:PID"`，`on_usb_connect` / `on_usb_disconnect` 指定输入源，支持按显示器分段配置。
- 关键设计：**每台电脑都装**，每台只负责"把屏幕切到自己"这一个方向。README 明确说"切走"有问题：如果另一台已让显示器休眠，显示器会立刻切回来。
- 局限：依赖硬件 USB 切换器；不管键鼠流转本身；不处理唤醒。

来源：[display-switch README](https://github.com/haimgel/display-switch#readme)。

### 2.3 必须正视的坑

| 坑 | 证据 | 对设计的影响 |
|---|---|---|
| 部分显示器只接受当前活跃输入口发来的 VCP 0x60 | [ddcutil issue #100](https://github.com/rockowitz/ddcutil/issues/100)、[ddcutil 显示器笔记](https://www.ddcutil.com/archived/monitor_notes/) | 切换命令必须委托给"当前占屏"的机器发出 → 必须有局域网控制面 |
| 输入源编码不标准（如某型号 HDMI-1 实际是 `0x05` 而非 `0x11`；能力串报的值不可信） | [ddcutil 显示器笔记](https://www.ddcutil.com/archived/monitor_notes/) | 需要"按显示器实测 + 用户校准"的输入源映射，不能只信能力串 |
| 新款 LG 不认标准 0x60，要走源地址 `0x50` + 私有 VCP `0xF4` | [ddcutil Wiki: LG 切源](https://github-wiki-see.page/m/rockowitz/ddcutil/wiki/Switching-input-source-on-LG-monitors) | 需要厂商适配层 |
| Windows DDC API 写死源地址 `0x51`，LG 方案在 Windows 上要绕到 NVAPI 直写 I2C | [meer-cha/lg-input-switch](https://github.com/meer-cha/lg-input-switch) | Windows 上 LG 支持依赖显卡厂商 API，成本高，可放到后期 |
| GetVCP 读到的当前输入要等新信号锁定后才更新；切换本身 1–3 秒 | [ddcutil 显示器笔记](https://www.ddcutil.com/archived/monitor_notes/)、[display-kvm README](https://github.com/lovezongzi/display-kvm) | 状态机要容忍延迟、做重试和超时 |
| USB 扩展坞、DisplayLink、部分 KVM/转接器会屏蔽 DDC | [input-switcher README](https://github.com/thiagocatao/input-switcher) | 首次启动要做兼容性自检 |
| Apple Silicon 内置 HDMI 口 DDC 支持不完整；Intel Mac 走另一套接口 | [m1ddc README](https://github.com/waydabber/m1ddc#readme) | macOS 端要两套后端 |

## 3. 已有的「切屏 + 切键鼠」组合项目（直接竞品）

| 项目 | 平台 | 许可证 | 做法 | 缺口 |
|---|---|---|---|---|
| [galactrex/Moniswitch](https://github.com/galactrex/Moniswitch) | Windows 为中心，Linux/mac 为客户端（mac 实验性） | MIT | Windows 端发 DDC；键鼠复用 Deskflow / Waynergy | Windows 必须是"中枢"且连着所有显示器 |
| [N1ckyChan/one-for-all-switch](https://github.com/N1ckyChan/one-for-all-switch) | 仅 Windows ↔ Windows | GPL-2.0 | 内嵌 Deskflow 1.26 子进程 + DDC；客户端请求主机切源 | 只支持 Win；Deskflow TLS 关闭 |
| [lovezongzi/display-kvm](https://github.com/lovezongzi/display-kvm) | Windows + Apple Silicon Mac | MIT | 各自独立键鼠，"碰哪台跳哪台"，DDC + 局域网委托 | 不共享键鼠 |
| [thiagocatao/input-switcher](https://github.com/thiagocatao/input-switcher) | 仅 Apple Silicon Mac | 未声明 | 快捷键切源 + 可选的 Mac 间命令中继 | 仅 mac、不共享键鼠 |
| [dayflower/dispsel](https://github.com/dayflower/dispsel) | macOS CLI | MIT | 除切源外还能控制**显示器内置 KVM**（USB 上行口切换） | 只是 CLI |
| [vladubogdan/logitech-monitor-switcher](https://github.com/vladubogdan/logitech-monitor-switcher)、[axle-h/software-kvm](https://github.com/axle-h/software-kvm)、[tommaso-zambon/unified-switch](https://github.com/tommaso-zambon/unified-switch) | 各异 | — | 监听罗技 Easy-Switch，跟随切 DDC | 绑定罗技硬件 |

**结论**：Win + mac + Linux 三平台对等、N 台机器、键鼠和画面一起走、带唤醒的组合，目前没有成熟实现。

两个值得借鉴的思路：

- **显示器内置 KVM**：不少带 USB-C 的显示器自带 KVM（USB 上行口跟随输入源切换），dispsel 已演示可以通过 DDC 控制。
  这种情况下连软件 KVM 都不需要，键鼠插在显示器上，切源即切键鼠，而且对睡眠中的 Mac 等于"真实 USB 按键"，能唤醒（未验证，需实测具体型号）。
- **绑定罗技 Easy-Switch**：键鼠硬件自己切频道，软件只负责跟着切屏。

## 4. 唤醒睡眠中的 Mac

| 方式 | 前提 | 可靠性 | 来源 |
|---|---|---|---|
| Wake-on-LAN 魔术包 | 有线以太网；系统设置开启"唤醒以供网络访问"，`pmset -g` 里 `womp 1`；同一子网广播 | 中偏低。Wi-Fi 基本不可用；Apple Silicon 上有失效报告；有人在 macOS Tahoe 26.4.1 更新后 Mac mini M2 有线 WoL 失效；唤醒后约 1 分钟会再睡回去 | [TidBITS 讨论](https://talk.tidbits.com/t/wake-on-lan-with-apple-silicon/33183)、[MacRumors 2026-04](https://forums.macrumors.com/threads/wol-not-working.2480810/)、[Reddit r/mac](https://www.reddit.com/r/mac/comments/1hv0os8/is_wakeonlan_broken_on_macos/) |
| USB HID 按键（真实或模拟的 USB 键盘） | 一个插在 Mac 上的 USB HID 设备 | 高，这是 Mac 的标准唤醒途径（常识，未找到 Apple 官方文档逐字说明） | — |
| 显示器内置 KVM 的 USB 上行切换 | 显示器带 KVM，键鼠插在显示器上 | 预计高（等价于 USB 插入/按键），未验证 | — |
| 不让它睡（"驻留"时持有 power assertion，如 `caffeinate -d`） | agent 常驻 | 高，但耗电，且显示器切走后 Mac 也可能被系统判定为空闲 | — |
| DDC 切源 / 显示器热插拔能否唤醒 Mac | — | 未找到证据，大概率不能，需实测 | — |

### 硬件 KVM / USB HID 注入项目（唤醒方案的参考）

| 项目 | 许可证 | Star | 最近提交 | 相关性 |
|---|---|---|---|---|
| [hrvach/deskhop](https://github.com/hrvach/deskhop) | GPL-3.0 | 7,929 | 2026-09-28 | RP2040 双 USB，键鼠在两台机器间切换；作为"真 USB 键鼠"可唤醒、可用于登录界面 |
| [pikvm/pikvm](https://github.com/pikvm/pikvm) | GPL-3.0 | 10,384 | 2026-10-13 | 树莓派 IP-KVM，USB HID gadget 注入 |
| [sipeed/NanoKVM](https://github.com/sipeed/NanoKVM) | GPL-3.0 | 6,555 | 2026-09-29 | 低价 RISC-V IP-KVM |
| [jetkvm/kvm](https://github.com/jetkvm/kvm) | GPL-2.0 | 5,249 | 2026-10-05 | 商业化 IP-KVM 固件 |
| [tiny-pilot/tinypilot](https://github.com/tiny-pilot/tinypilot) | MIT | 3,481 | 2026-08-17 | 树莓派浏览器 KVM |

IP-KVM 是"远程看画面"方案，对我们太重；值得借鉴的是 **DeskHop 这种"小 MCU 冒充 USB 键鼠"** 的思路：
做一个可选配件（ESP32-S3 / RP2040，十几到几十元），插在 Mac 上，通过局域网或 BLE 收到指令后发一次按键，用来可靠唤醒。

## 5. 产品方向建议

**定位**：「一键把整张桌子交给另一台电脑」——画面（DDC 切源）+ 键鼠（软件 KVM）+ 唤醒，Win / mac / Linux 三平台对等，支持 N 台机器。

**架构要点**

1. **每台机器一个 agent，局域网组网**（mDNS 发现 + 配对 + 加密通道）。
2. **显示器所有权状态机**：记录当前谁占屏；切换请求一律路由给占屏者发 DDC，占屏者发不出时再由目标机尝试兜底。
3. **DDC 适配层**：Windows（Monitor Configuration API）、Linux（i2c-dev，可参考 ddcutil）、macOS Apple Silicon（IOAVService 私有接口，参考 m1ddc）、Intel Mac（IOKit I2C），加一张厂商怪癖表（LG `0xF4`、非标准输入编码）和用户校准流程。
4. **键鼠层**：MVP 先以独立进程方式驱动 Deskflow（成熟、三平台、TLS），后期再考虑自研或对接 Lan Mouse。
5. **唤醒层**：WoL 作为免费基础能力 + 可选 USB HID 配件作为可靠方案 + "驻留不睡"开关。

**技术栈倾向**（待定）：Rust 核心（display-switch、Lan Mouse、ddc-hi 都是 Rust，生态最近）+ Tauri 做托盘 / 设置界面。

## 6. 命名

### 6.1 要避开的词

- **Flow**：罗技在同一场景（多电脑键鼠流转）使用 Flow™ 商标，见 [Logitech Flow 官网](https://www.logitech.com/en-eu/software/flow)；Deskflow 也在用。
  `SystemFlow` / `InputFlow` 都会被联想成罗技或 Deskflow 的附属物，`SystemFlow` 本身也太泛（GitHub 上 31 个同名仓库，都与本领域无关）。
- **接力**：苹果 Handoff 的中文官方名就是"接力"。
- **Hop**：DeskHop（7.9k 星）已在同一领域。
- **Crossdesk**：已被一个 4.5k 星的远程桌面项目 [kunkundi/crossdesk](https://github.com/kunkundi/crossdesk) 占用。
- **DeskFerry**：已是商业 AI 产品 [deskferry.com](https://deskferry.com/)。

### 6.2 候选名查重（GitHub 仓库名搜索 + 网页搜索，2026-10-16）

| 名字 | 寓意 | 查重结果 | 风险 |
|---|---|---|---|
| **DeskBaton** | 桌面接力棒：一键把整张桌子"交棒"给下一台 | GitHub 0 个同名仓库；网页只找到无关的 ShiftBaton（餐饮排班） | 低 |
| **SeatShift** | 换座：同一个座位，换一台电脑坐 | GitHub 0 个；有荷兰票务网站 SeatShift 和一个 itch.io 小游戏，非软件工具领域 | 低–中 |
| **KVMHop** | 直白描述功能 | GitHub 0 个；但与同领域的 DeskHop 易混 | 中 |
| DeskShift | 桌面切换 | GitHub 2 个 0 星仓库 | 低–中 |
| Switchyard | 调车场 | 被 NVIDIA-NeMo/Switchyard（3.4k 星）占用 | 高 |
| SystemFlow | 系统流转 | 罗技 Flow™ 联想 + 名字过泛 | 高 |

中文名可以配"交棒"（与 DeskBaton 呼应）或"换座"（与 SeatShift 呼应），中文名的商标未做检索。

**注意**：以上只查了 GitHub 和网页搜索，**没有查商标数据库**（中国商标网 / USPTO / EUIPO），确定名字前要补查。

## 7. 已确认的决定（2026-10-16）

| 事项 | 决定 |
|---|---|
| 产品形态 | 纯软件核心（局域网协同 + DDC 切源 + 驱动 Deskflow 管键鼠），唤醒用 WoL 兜底，另配可选 USB HID 唤醒配件 |
| 产品名 | DeskBaton（中文：交棒）；`systemflow` 仅作内部代号 |
| 首版平台 | Windows + Apple Silicon Mac，两台机器 |
| 许可证 | 开源，GPL；可直接复用 Deskflow / Lan Mouse 代码 |
| 键鼠接法 | 插在显示器自带的 USB 口 / KVM 上，切输入源时键鼠跟随；软件 KVM 降为可选 |
| Mac 睡眠策略 | 不长期阻止睡眠；交棒后只短时保持唤醒，长时间睡眠后靠唤醒链（HID 配件等）接管 |
| 技术栈 | Rust 单代码库 + macOS ObjC 薄胶水层 |

## 8. 用户实际硬件分析：泰坦军团 P275MV（Mini-LED）

**设备与接法**：Windows 台式机（有线网，USB-B 上行）；MacBook Pro（通常只连 Wi-Fi，一根全功能 USB-C 同时传画面、USB 和供电）。

### 8.1 显示器已知规格

| 项目 | 内容 | 来源 |
|---|---|---|
| 接口 | 2×HDMI 2.1、1×DP 1.4、1×USB-C（65W PD）、2×USB-A、USB-B 上行 | [PChome](https://article.pchome.net/content-2174998.html)、[IT之家](https://www.ithome.com/0/764/853.htm) |
| KVM | 内置 KVM，一套键鼠控制两台主机 | [PChome](https://article.pchome.net/content-2174998.html) |
| 型号区分 | P275MV MAX 版的 USB-C 为 90W 且带 KVM；PLUS 版据 igor'sLAB 评测**没有 KVM** | [新浪众测](https://zhongce.sina.com.cn/iframe/article/view/191392/)、[igor'sLAB](https://www.igorslab.de/en/titan-army-p275mv-plus-tested-miniled-dual-mode-performance-brightness-all-rounder/2/) |
| KVM 行为（MAX 版社区实测） | **USB 自动跟随视频输入**：显示 DP 时键鼠走 USB-B，显示 USB-C 时键鼠走 USB-C；菜单里的"USB Switch"基本用不上 | [sizu.me 博客](https://sizu.me/seak/posts/0hchws81zbxr) |
| 键鼠唤醒 | 开启菜单项"USB Power(Sleep)"后，可用键鼠唤醒睡眠中的电脑 | 同上 |
| 无信号关机 | 输入断开约 10 秒后显示器会进入关机状态，此时菜单按钮不好操作 | [sizu.me 博客](https://sizu.me/seak/posts/8ra0fh5te03k) |
| DDC/CI 输入源编码 | 未找到资料，需实测；USB-C 常见编码为 `0x1B`（27） | [jensenbox/kvm-switch](https://github.com/jensenbox/kvm-switch)、[m1ddc README](https://github.com/waydabber/m1ddc#readme) |

### 8.2 对设计的影响

1. **如果 KVM 确实跟随输入源，切输入源就等于切键鼠**，首版不需要软件 KVM，也不依赖 Deskflow。
   [jensenbox/kvm-switch](https://github.com/jensenbox/kvm-switch) 在技嘉 M32U 上验证过同样的模式（Linux 走 DP、Mac 走 USB-C）。
2. **"谁来发切换命令"的问题消失了**：键鼠永远连在当前占屏的那台机器上，热键由它接收，DDC 也由它发，正好满足"只能从活跃输入口发命令"的限制。
   两台机器各装一个程序、注册同一个热键即可。
3. **唤醒问题换了个形态**：
   - 切到睡着的 Mac：键鼠已经被 KVM 切到 Mac 上，按一下键理论上就能唤醒（等价于真实 USB 键盘），**USB 唤醒配件可能完全不需要**。待实测。
   - 风险：Mac 没信号时，如果显示器开着"自动选择输入源"，可能自己跳回 DP，需要在菜单里关掉。
   - Mac 只连 Wi-Fi，**WoL 不可用**。替代方案是官方设置"接电源适配器且显示器关闭时，防止自动进入睡眠"（[Apple 支持文档](https://support.apple.com/guide/mac-help/set-sleep-and-wake-settings-mchle41a6ccd/mac)）。Mac 由显示器 USB-C 供电，算"接电源"，系统一直醒着、程序随时在线，代价是耗电。
   - 切到睡着的 Windows：Windows 走有线网，Mac 端可以先发 WoL，再切输入源；或者切过去后按键鼠唤醒。
4. **合盖（clamshell）风险**：如果 MacBook 合盖使用，切走输入源时 macOS 可能认为外接显示器断开，没有任何屏幕就会立刻睡眠。需实测。
5. **供电**：65W PD 对 14 英寸 MacBook Pro 够用，16 英寸高负载下可能会掉电（推断，未验证）。

### 8.3 合盖 MacBook 切走后"按键唤不醒、必须开盖"的根因

- 用户确认：Windows 走 DP + USB-B；MacBook 走 USB-C，**平时合盖使用**；显示器暂时无法调设置。
- **合盖模式的成立条件**：Apple 官方要求合盖的 Mac 必须同时接着电源、外接显示器、外接键盘和鼠标（[Apple 支持：外接显示器黑屏](https://support.apple.com/en-us/102501)）。
  这些条件满足时，macOS 会自动把 `AppleClamshellCausesSleep` 置为 false；外接显示器一消失，就退回普通的合盖睡眠（[clamshell.dev 原理说明](https://clamshell.dev/guides/how-keep-awake-apps-work-lid-closed)）。
- **触发**：切到 Windows 时，显示器 KVM 把键鼠切走，USB-C 视频也不再是活跃输入，Mac 很可能同时"失去"键鼠和显示器。合盖条件不再满足，于是 Mac 进入合盖睡眠。
- **软件常规手段拦不住**：合盖睡眠是内核 `IOPMrootDomain` 处理的硬件事件，`caffeinate` 这类空闲睡眠断言否决不了（[orca issue #20194](https://github.com/stablyai/orca/issues/20194)）。
  能拦住的只有三种：
  - 需要 root 的 `pmset -a disablesleep 1`；
  - IOKit 的合盖睡眠状态控制，据 Clamshell 这款应用的说法**不需要管理员权限**（未独立验证）；
  - HDMI 假显示器插头。
- **切回来按键也唤不醒**（推断，未验证）：键鼠是在 Mac 睡着之后才被 KVM 接过来的，大概率没被登记为唤醒源，所以只能开盖。
- **结论**：与其想办法"唤醒"，不如让 Mac 在交出控制权之前就进入"驻留不睡"状态。
