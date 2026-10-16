use baton_core::config::WakeMethod;
use baton_core::ports::{WakeOutcome, WakeTarget, Waker};
use baton_core::{Error, Result};

/// 常插在目标电脑上的 USB / BLE HID 配件（ESP32-S3 / RP2040），收到指令后模拟一次按键。
pub struct HidDongleWaker;

impl Waker for HidDongleWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::HidDongle
    }

    fn wake(&self, _target: &WakeTarget) -> Result<WakeOutcome> {
        Err(Error::Unsupported("HID 唤醒配件"))
    }
}
