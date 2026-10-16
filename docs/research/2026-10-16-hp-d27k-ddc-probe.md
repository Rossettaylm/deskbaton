# HP D27k DDC 与 macOS 电源接口实测（2026-10-16）

环境：Mac mini（Mac16,11，Apple M4 Pro），macOS 26；两台 HP D27k 外接。这不是目标硬件（P275MV + 合盖 MacBook Pro），只用来验证 DDC 实现和接口可调用性。

## IORegistry 结构

- 每台外接显示器对应一个 `IOMobileFramebufferShim`（`dispextN`），属性里有 `EDID UUID` 和 `DisplayAttributes.ProductAttributes`（`ManufacturerID`、`ProductName`、`AlphanumericSerialNumber`、`SerialNumber`）。没接显示器的 framebuffer 没有 `EDID UUID`。
- 对应的 `DCPAVServiceProxy`（`Location = External`）不在 framebuffer 子树里，而是在 `dcpextN` 子树里，遍历顺序上紧跟在 framebuffer 之后。实现按遍历顺序配对。
- 两台同型号显示器的 EDID UUID 不同（`…-0103803C2278` 与 `…-0104B53C2278`），但不能保证所有同型号显示器都不同，所以重复时追加 `#序号`。

## 读取输入源（VCP `0x60`）

| 显示器（序列号） | m1ddc 1.2.0 | deskbaton | 说明 |
|---|---|---|---|
| BVTB42550236 | `0` | `0x11`（HDMI 1） | 回包校验和正确；m1ddc 不校验回包，`0` 不是合法编码 |
| BVTB42350197 | `15`（`0x0F`，DP） | `0x0F` | 一致 |

- 每次读取约 80 毫秒（含写请求后等待 50 毫秒），连续 3 轮都是第一次就成功。
- 回包格式与 DDC/CI 规范一致：`6E 88 02 结果 VCP 类型 最大值(2) 当前值(2) 校验和`，校验和为 `0x50` 与前 10 字节异或。

## 切走再切回

对第一台执行 `set-input 0x0F` 等 5 秒再 `set-input 0x11`，用户反馈「看起来 ok」。**待确认**：是自动切回（显示器在非活跃输入口上仍接受 DDC），还是用户手动切回。无论结果如何，设计仍按「显示器只接受活跃输入口的 DDC」处理。

## macOS 电源接口

- `IOServiceOpen(IOPMrootDomain)` + `IOConnectCallScalarMethod(kPMSetClamshellSleepState = 12)`：普通用户权限返回成功，设 1、设 0 都可以。Mac mini 没有盖子，合盖效果需要在 MacBook 上验收。
- `IOPMAssertionCreateWithName(PreventUserIdleSystemSleep)`：成功，`pmset -g assertions` 能看到。断言名称在 `CFSTR` 中使用中文时显示为空，改用 ASCII。
- `IOPMAssertionDeclareUserActivity(kIOPMUserActiveLocal)`：普通用户权限返回成功。
