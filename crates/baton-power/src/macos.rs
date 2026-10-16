use std::time::Duration;

use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};
use baton_macos_sys as sys;

/// 合盖 MacBook 的睡眠控制：交棒后短时阻止合盖睡眠，到期后允许系统正常睡眠。
pub struct MacPower;

impl PowerControl for MacPower {
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard> {
        if duration.is_zero() {
            return Ok(AwakeGuard::noop());
        }
        let secs = u32::try_from(duration.as_secs()).unwrap_or(u32::MAX);
        // SAFETY: 只传值参数。
        check(unsafe { sys::deskbaton_stay_awake_begin(secs) }, "保持唤醒")?;
        Ok(AwakeGuard::new(|| {
            // SAFETY: 无参数；重复调用由原生侧保证幂等。
            unsafe { sys::deskbaton_stay_awake_end() };
        }))
    }

    fn wake_display(&self) -> Result<()> {
        // SAFETY: 无参数。
        check(unsafe { sys::deskbaton_wake_display() }, "显示唤醒")
    }
}

fn check(rc: i32, what: &'static str) -> Result<()> {
    match rc {
        sys::OK => Ok(()),
        sys::ERR_UNIMPLEMENTED => Err(Error::Unsupported(what)),
        other => Err(Error::Power(format!("{what}：原生错误码 {other}"))),
    }
}
