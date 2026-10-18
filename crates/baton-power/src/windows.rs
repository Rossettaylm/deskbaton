use std::sync::Mutex;
use std::time::Duration;

use baton_core::ports::{AwakeGuard, PowerControl};
use baton_core::{Error, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Power::{
    ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, PowerClearRequest, PowerCreateRequest,
    PowerRequestSystemRequired, PowerSetRequest, SetThreadExecutionState,
};
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};
use windows::core::w;

use crate::switch::SharedSwitch;

/// 拦住系统睡眠；显示输出由共用屏另一台电脑负责，不额外要求显示器常亮。
pub struct WinPower;

/// PowerRequest 句柄是进程内对象，经 Mutex 串行访问。
struct Request(HANDLE);

// SAFETY: 只经 `REQUEST` 这把锁读写，跨线程传递句柄值本身。
unsafe impl Send for Request {}
// SAFETY: 同上，从不在锁外同时使用。
unsafe impl Sync for Request {}

static REQUEST: Mutex<Option<Request>> = Mutex::new(None);
static STAY_AWAKE: SharedSwitch = SharedSwitch::new(begin, end);

impl PowerControl for WinPower {
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard> {
        if duration.is_zero() {
            return Ok(AwakeGuard::noop());
        }
        STAY_AWAKE.hold(duration)
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

fn begin() -> Result<()> {
    let context = REASON_CONTEXT {
        Version: 0,
        Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
        Reason: REASON_CONTEXT_0 {
            SimpleReasonString: windows::core::PWSTR(w!("DeskBaton").as_ptr().cast_mut()),
        },
    };
    // SAFETY: context 只在本次调用有效；字符串是程序生命周期内的字面量。
    let handle = unsafe { PowerCreateRequest(&raw const context) }
        .map_err(|e| Error::Power(format!("无法创建唤醒请求：{e}")))?;
    // SAFETY: handle 刚由 PowerCreateRequest 返回。
    if let Err(e) = unsafe { PowerSetRequest(handle, PowerRequestSystemRequired) } {
        // SAFETY: 尚未交给其它代码，失败则关掉。
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(Error::Power(format!("无法阻止系统睡眠：{e}")));
    }
    *lock() = Some(Request(handle));
    Ok(())
}

fn end() {
    let Some(Request(handle)) = lock().take() else {
        return;
    };
    // SAFETY: handle 由 begin 创建且尚未关闭。
    unsafe {
        let _ = PowerClearRequest(handle, PowerRequestSystemRequired);
        let _ = CloseHandle(handle);
    }
}

fn lock() -> std::sync::MutexGuard<'static, Option<Request>> {
    REQUEST.lock().unwrap_or_else(|e| e.into_inner())
}
