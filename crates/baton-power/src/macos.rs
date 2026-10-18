use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use baton_core::event::Event;
use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};
use baton_macos_sys as sys;

use crate::sleep_helper;
use crate::switch::SharedSwitch;
use crate::wake_reason::is_network_wake_reason;

static WAKE_TX: Mutex<Option<Sender<()>>> = Mutex::new(None);

static STAY_AWAKE: SharedSwitch = SharedSwitch::new(begin, end);

/// 合盖 MacBook 的睡眠控制：经 sudoers 规则置 `SleepDisabled`，并持有防空闲睡眠与防显示器睡眠断言。
pub struct MacPower;

/// IOKit 在专用线程的 CFRunLoop 上调用；只向通道发信号。
unsafe extern "C" fn on_system_wake() {
    if let Ok(guard) = WAKE_TX.lock()
        && let Some(tx) = guard.as_ref()
    {
        let _ = tx.send(());
    }
}

pub(crate) fn spawn_network_wake_watch(events: Sender<Event>) {
    let (tx, rx) = mpsc::channel();
    *WAKE_TX.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
    thread::spawn(|| {
        // SAFETY: 回调只向通道发信号。
        let rc = unsafe { sys::deskbaton_watch_wake(Some(on_system_wake)) };
        if rc != sys::OK {
            log::warn!("无法订阅系统唤醒：原生错误码 {rc}");
        }
    });
    thread::spawn(move || {
        while rx.recv().is_ok() {
            if is_network_wake_reason(&kern_wakereason())
                && events.send(Event::NetworkWake).is_err()
            {
                break;
            }
        }
    });
}

fn kern_wakereason() -> String {
    Command::new("/usr/sbin/sysctl")
        .args(["-n", "kern.wakereason"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

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
        let marker = marker();
        if !marker.exists() || STAY_AWAKE.is_on() {
            return Ok(());
        }
        sleep_helper::set_sleep_disabled(false)?;
        let _ = fs::remove_file(marker);
        Ok(())
    }
}

/// `SleepDisabled` 重启后仍然保留，标记必须放在不会被清理的位置，进程异常退出、甚至重启后才能据此恢复。
fn marker() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    marker_in(home.as_deref())
}

fn marker_in(home: Option<&Path>) -> PathBuf {
    match home {
        Some(home) => home.join("Library/Application Support/DeskBaton/stay-awake"),
        None => std::env::temp_dir().join("deskbaton-stay-awake"),
    }
}

fn begin() -> Result<()> {
    // SAFETY: 无参数。
    check(unsafe { sys::deskbaton_stay_awake_begin() }, "保持唤醒")?;
    match sleep_helper::sleep_disabled() {
        // 用户自己设置的值不归我们管，也就不留标记、不在结束时恢复。
        Ok(true) => return Ok(()),
        Ok(false) => {}
        Err(e) => log::warn!("读取 SleepDisabled 失败，按未设置处理：{e}"),
    }
    let marker = marker();
    if let Some(dir) = marker.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Err(e) = fs::write(&marker, std::process::id().to_string()) {
        log::warn!("写入睡眠恢复标记失败，异常退出后可能需要手动恢复：{e}");
    }
    if let Err(e) = sleep_helper::set_sleep_disabled(true) {
        let _ = fs::remove_file(&marker);
        log::warn!("只阻止了空闲睡眠，合盖的 Mac 切回时仍会睡眠：{e}");
    }
    Ok(())
}

fn end() {
    // SAFETY: 无参数，可重复调用。
    unsafe { sys::deskbaton_stay_awake_end() };
    let marker = marker();
    if !marker.exists() {
        return;
    }
    match sleep_helper::set_sleep_disabled(false) {
        Ok(()) => {
            let _ = fs::remove_file(marker);
        }
        Err(e) => log::warn!("恢复 SleepDisabled 失败，下次启动时重试：{e}"),
    }
}

fn check(rc: i32, what: &'static str) -> Result<()> {
    match rc {
        sys::OK => Ok(()),
        sys::ERR_UNIMPLEMENTED => Err(Error::Unsupported(what)),
        other => Err(Error::Power(format!("{what}：原生错误码 {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_survives_reboot_when_home_is_known() {
        assert_eq!(
            marker_in(Some(Path::new("/Users/me"))),
            Path::new("/Users/me/Library/Application Support/DeskBaton/stay-awake")
        );
        assert!(marker_in(None).starts_with(std::env::temp_dir()));
    }
}
