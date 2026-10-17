//! 本机键鼠活动：距离最近一次输入多久。

use std::time::Duration;

use baton_core::Result;
use baton_core::ports::InputActivity;

pub struct PlatformActivity;

impl InputActivity for PlatformActivity {
    fn idle(&self) -> Result<Duration> {
        idle()
    }
}

#[cfg(target_os = "macos")]
fn idle() -> Result<Duration> {
    use baton_macos_sys as sys;
    // SAFETY: 纯查询函数，参数为 CoreGraphics 定义的常量。
    let secs = unsafe {
        sys::CGEventSourceSecondsSinceLastEventType(
            sys::CG_EVENT_SOURCE_STATE_HID_SYSTEM,
            sys::CG_ANY_INPUT_EVENT_TYPE,
        )
    };
    Duration::try_from_secs_f64(secs)
        .map_err(|_| baton_core::Error::Unsupported("读取键鼠空闲时长"))
}

#[cfg(target_os = "windows")]
fn idle() -> Result<Duration> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `cbSize` 已设置为结构体大小，系统只写入 `info`。
    if !unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        return Err(baton_core::Error::Unsupported("读取键鼠空闲时长"));
    }
    // SAFETY: 无参数。
    let now = unsafe { GetTickCount() };
    // 两者都是开机以来的毫秒数，约 49.7 天回绕一次，用回绕减法。
    Ok(Duration::from_millis(u64::from(
        now.wrapping_sub(info.dwTime),
    )))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn idle() -> Result<Duration> {
    Err(baton_core::Error::Unsupported("当前操作系统的键鼠活动检测"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn reports_idle_time() {
        let first = PlatformActivity.idle().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert!(PlatformActivity.idle().unwrap() >= first);
    }
}
