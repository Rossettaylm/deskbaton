# 合盖 Mac 的 Wi-Fi 魔术包唤醒实测（2026-10-16 ~ 10-17）

环境：MacBook Pro 14"（Mac15,7，M3 Pro），macOS 15.7.4，合盖，单根 USB-C 接 P275MV 供电（AC）；Wi-Fi 网卡 `en0`（Broadcom `0x14E4, 0x4388`），5 GHz 802.11ax；Windows 台式机有线，同一网段 `192.168.31.0/24`。局域网里没有 Apple TV / HomePod 等 Bonjour 睡眠代理。

## 结论

- **Windows 发出的魔术包能把合盖睡眠的 Mac 唤醒，不需要配件、不需要睡眠代理、不需要宣告 Bonjour 服务。** 睡 45 秒和睡约 30 分钟（期间经历一次系统维护唤醒）都成功。
- 唤醒先进入暗唤醒（`DarkWake ... due to smc.70070000 wifibt`）。随即声明用户活动（`IOPMAssertionDeclareUserActivity`，即 `wake_display`）即转为完整唤醒、点亮显示输出；再置 `SleepDisabled=1`，之后用 OSD 切回 USB-C，键鼠回到 Mac，不再睡着。与设计 6.2 第 2–4 步的顺序一致。
- 魔术包的目标 MAC 必须是 `en0` **当前在用**的地址。本机该网络启用了私有 Wi-Fi 地址：在用 `92:4b:c9:cd:0e:00`，硬件地址 `80:a9:97:2b:09:c2`。私有地址设为「轮换」时会变。

## 前提与判据

| 项 | 本机状态 | 来源 |
|---|---|---|
| 「唤醒以供网络访问」 | `pmset -g` 中 `womp 1` | `man pmset`：womp 与该设置等价 |
| Wi-Fi 硬件支持网络唤醒 | `system_profiler SPAirPortDataType` 显示 `Wake On Wireless: Supported` | Apple Remote Desktop 文档给出的检查方法 |
| 网卡驱动已启用魔术包唤醒 | `pmset -g assertions` 中内核断言 `0x100=MAGICWAKE description=en0 owner=IOSkywalkNetworkBSDClient` | XNU `IOPM.h`：`kIOPMDriverAssertionMagicPacketWakeEnabledBit = 0x100`，「driver is informing PM that magic packet wake is enabled」 |
| 本次唤醒来自 Wi-Fi | 唤醒后 `sysctl kern.wakereason` 含 `wifibt`；airportd 日志 `systemWokenByWiFi: ... was woken by WiFi, was not woken by Bluetooth` | 实测 |

**不能作为判据**：airportd 入睡时的 `WoW: _wowEnabled: WoW is not active on en0` 和 `Scan offloads ... wow=disabled`。四轮实测里它每次都这样报，Mac 照样被魔术包唤醒。

## 实测记录

脚本：Mac 端 `scripts/probe/mac-wol-wake.sh`，Windows 端 `scripts/probe/win-send-wol.ps1`（由 Mac 端经临时 HTTP 服务下发，`irm http://<Mac IP>:8765 | iex`）。Windows 取走脚本后 Mac 等 10 秒 `pmset sleepnow`；Windows 等 `-w` 秒后向 `255.255.255.255`、子网广播、Mac 单播 IP 的 UDP 9 各发 3 个魔术包。Mac 醒来后模拟 agent：`caffeinate -u` + `sudo -n pmset -a disablesleep 1`。

| 轮次 | 条件 | 结果 |
|---|---|---|
| 1（00:05） | 不宣告服务 | 没有唤醒，开盖才醒。**作废**：Windows 的 `irm` 卡十几秒后报连不上，推断是 Mac 防火墙询问 python3 期间连接超时，用户点允许后服务端才接受这个已失效的连接，Mac 误记为「已取走」，Windows 实际没有发包 |
| 2（10:53） | 宣告 `_deskbaton._tcp`，等 60 秒 | 10:53:31 取走，10:54:32 `DarkWake ... wifibt`，同秒转完整唤醒；26 秒后键鼠回到 Mac，未再睡 |
| 3（11:01） | 不宣告服务（对照），等 60 秒 | 11:01:28 取走，11:02:29 `wifibt` 暗唤醒；31 秒后键鼠回到 Mac，未再睡 |
| 4（11:05） | 宣告服务，等 1800 秒 | **作废**：11:22:09 系统维护唤醒（`rtc/SleepService`），旧脚本误当作目标唤醒并点亮显示；30 秒后显示关闭但系统没睡，约 11:35 发包时 Mac 醒着、显示输出关着，切回后 KVM 不交键鼠（即 `TODO.md` 中 displaysleep 那一项的死循环），只能开盖 |
| 5（11:42） | 宣告服务，等 1800 秒，脚本只认 `wifibt` / `lid` | 11:42:16 入睡；11:58:50 维护唤醒（`rtc/Maintenance`），被忽略，46 秒后自行睡回；**12:12:37 `wifibt` 暗唤醒**，同秒转完整唤醒，17 秒后键鼠回到 Mac，未再睡。下一次维护唤醒排在 12:15:17，可排除是它 |

第 2、3 轮的唤醒时刻都在「取走 + 60 秒」后约 1 秒。第 5 轮比「取走 + 1800 秒」晚 37 秒，与 Windows 端 360 次 ping 的累计耗时吻合；Windows 窗口里「已发魔术包」的时刻未记录。

## mDNSResponder 与 Bonjour 服务

`mDNSCore/mDNS.c` 的 `BeginSleepProcessing`：`!m->SystemWakeOnLANEnabled` 或 `!mDNSCoreHaveAdvertisedMulticastServices(m)`（本机没有宣告任何 SRV 记录）时直接跳过睡眠代理注册。第 3 轮对照说明这条路径与魔术包唤醒无关：不宣告服务也能唤醒。当前开源版本已不含 macOS 平台层（`mDNSMacOSX`），网卡内代理的配置无法从源码确认。

## 坑

- **系统维护唤醒**：本机睡眠后约每 15–17 分钟有一次 `rtc` 暗唤醒（`dasd` 的 `SleepService` / `Maintenance`，`pmset -g log` 的 `Wake Requests` 可见计划时间），持续几十秒后自行睡回。任何「检测到醒来就点亮」的逻辑都要先看 `kern.wakereason`。
- **区分魔术包与其他 Wi-Fi 唤醒**：内核不记录唤醒包（`net.link.generic.system.port_used` 下没有对应信息），推送等其他流量也可能以 `wifibt` 唤醒，只能靠发包时间对照。
- **bash 3.2 多字节字符**：`"$VAR："` 会把全角标点首字节读进变量名，`set -u` 下报 unbound variable，要写成 `${VAR}：`。

## 对设计的含义

- 唤醒链里的 `Wol` 对合盖的 Mac 同样有效，免配件唤醒有了；HID 配件降为备选。
- Mac 的 `wol_mac` 应由 agent 在 `hello` 里上报 `en0` 当前地址（现在写死为 `None`），首次运行时提示把该网络的私有 Wi-Fi 地址设为「固定」。
- 本轮实测分不出广播和单播哪一路生效，`WolWaker` 宜三路都发。
- **未验证**：魔术包触发的暗唤醒在没人干预时能持续多久。四轮实测中脚本都在同一秒把它转成了完整唤醒；真实流程里要等 agent 重连、Windows 收到心跳后发 `prepare`，Mac 才点亮。暗唤醒太短会错过。维护唤醒能持续约 46 秒，可作参考。
- **未验证**：睡眠超过 1 小时（验收清单第 2 项）、电池供电、`hibernatemode 3` 下进入 standby 后网卡是否仍在线。
