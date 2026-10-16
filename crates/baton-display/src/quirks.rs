/// 显示器切换输入源所用的 DDC 命令形式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputSwitchMethod {
    /// 标准 MCCS：VCP `0x60`，主机源地址 `0x51`。
    #[default]
    StandardVcp60,
    /// 新款 LG：私有 VCP `0xF4`，源地址 `0x50`。Windows 的 Dxva2 无法发送，需要显卡厂商 I2C 接口。
    LgVcpF4,
}
