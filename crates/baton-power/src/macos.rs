use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};
use baton_macos_sys as sys;

use crate::switch::SharedSwitch;

static STAY_AWAKE: SharedSwitch = SharedSwitch::new(begin, end);

/// 合盖 MacBook 的睡眠控制：经 IOKit 关闭合盖睡眠并持有防空闲睡眠断言，guard 释放或到期后恢复。
pub struct MacPower;

impl PowerControl for MacPower {
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard> {
        if duration.is_zero() {
            return Ok(AwakeGuard::noop());
        }
        STAY_AWAKE.hold(duration)
    }

    fn wake_display(&self) -> Result<()> {
        // SAFETY: 无参数。
        check(unsafe { sys::deskbaton_wake_display() }, "显示唤醒")
    }

    fn restore_sleep(&self) -> Result<()> {
        if !marker().exists() || STAY_AWAKE.is_on() {
            return Ok(());
        }
        // SAFETY: 无参数，可重复调用。
        check(unsafe { sys::deskbaton_stay_awake_end() }, "恢复睡眠设置")?;
        remove_marker();
        Ok(())
    }
}

/// 进程异常退出时系统可能仍处于禁止合盖睡眠状态，下次启动据此恢复。
fn marker() -> PathBuf {
    std::env::temp_dir().join("deskbaton-stay-awake")
}

fn remove_marker() {
    let _ = fs::remove_file(marker());
}

fn begin() -> Result<()> {
    let _ = fs::write(marker(), std::process::id().to_string());
    // SAFETY: 无参数。
    let result = check(unsafe { sys::deskbaton_stay_awake_begin() }, "保持唤醒");
    if result.is_err() {
        remove_marker();
    }
    result
}

fn end() {
    // SAFETY: 无参数，可重复调用。
    if unsafe { sys::deskbaton_stay_awake_end() } == sys::OK {
        remove_marker();
    }
}

fn check(rc: i32, what: &'static str) -> Result<()> {
    match rc {
        sys::OK => Ok(()),
        sys::ERR_UNIMPLEMENTED => Err(Error::Unsupported(what)),
        other => Err(Error::Power(format!("{what}：原生错误码 {other}"))),
    }
}
