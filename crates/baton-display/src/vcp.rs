//! 用到的 MCCS VCP 编码，以及与平台无关的读数。

#[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
pub const INPUT_SOURCE: u8 = 0x60;
pub const BRIGHTNESS: u8 = 0x10;
pub const VOLUME: u8 = 0x62;
/// `1` 静音，`2` 取消静音。
pub const MUTE: u8 = 0x8D;
pub const MUTE_ON: u16 = 1;
pub const MUTE_OFF: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VcpValue {
    pub current: u16,
    pub max: u16,
}

/// 显示器明确回「不支持」与读到值分开；读写失败走 `Err`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcpRead {
    #[cfg_attr(not(any(target_os = "macos", target_os = "windows")), allow(dead_code))]
    Value(VcpValue),
    Unsupported,
}
