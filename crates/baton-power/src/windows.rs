use std::time::Duration;

use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};

pub struct WinPower;

impl PowerControl for WinPower {
    fn stay_awake(&self, _duration: Duration) -> Result<AwakeGuard> {
        Ok(AwakeGuard::noop())
    }

    fn wake_display(&self) -> Result<()> {
        Err(Error::Unsupported("Windows 显示唤醒"))
    }

    fn restore_sleep(&self) -> Result<()> {
        Ok(())
    }
}
