use std::time::Duration;

use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};
use windows::Win32::System::Power::{
    ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
};

/// Windows 台式机不需要合盖保持唤醒，只负责接棒前点亮显示输出。
pub struct WinPower;

impl PowerControl for WinPower {
    fn stay_awake(&self, _duration: Duration) -> Result<AwakeGuard> {
        Ok(AwakeGuard::noop())
    }

    fn wake_display(&self) -> Result<()> {
        // SAFETY: 只传标志位；不带 ES_CONTINUOUS，只重置一次空闲计时，不改变持续状态。
        let previous = unsafe { SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) };
        if previous.0 == 0 {
            return Err(Error::Power("点亮显示失败".into()));
        }
        Ok(())
    }

    fn restore_sleep(&self) -> Result<()> {
        Ok(())
    }
}
