//! 本机电源控制（短时保持唤醒、点亮显示）与唤醒对端的各种方式。

pub mod sleep_helper;
pub mod wake;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod switch;
#[cfg(target_os = "windows")]
mod windows;

use baton_core::ports::{PowerControl, Waker};

pub fn platform_power() -> Box<dyn PowerControl> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacPower);
    #[cfg(target_os = "windows")]
    return Box::new(windows::WinPower);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(unsupported::UnsupportedPower);
}

/// 当前平台可用的全部唤醒方式，由 [`baton_core::wake::WakeChain`] 按配置挑选排序。
pub fn available_wakers() -> Vec<Box<dyn Waker>> {
    vec![
        Box::new(wake::WolWaker::default()),
        Box::new(wake::HidDongleWaker),
        Box::new(wake::PromptWaker),
    ]
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use std::time::Duration;

    use baton_core::ports::{AwakeGuard, PowerControl};
    use baton_core::{Error, Result};

    pub struct UnsupportedPower;

    impl PowerControl for UnsupportedPower {
        fn stay_awake(&self, _: Duration) -> Result<AwakeGuard> {
            Err(Error::Unsupported("当前操作系统的睡眠控制"))
        }
        fn wake_display(&self) -> Result<()> {
            Err(Error::Unsupported("当前操作系统的显示唤醒"))
        }
        fn restore_sleep(&self) -> Result<()> {
            Ok(())
        }
    }
}
