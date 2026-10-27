//! macOS 原生胶水的 FFI 声明，对应 `native/deskbaton_native.h`。
//! 返回值约定：`0` 成功，负数为错误码。

#![cfg(target_os = "macos")]

use std::ffi::c_char;

pub const OK: i32 = 0;
pub const ERR_UNIMPLEMENTED: i32 = -1;
pub const ERR_NOT_FOUND: i32 = -2;
pub const ERR_IO: i32 = -3;

/// 对应 `deskbaton_display_info`，字段均为以 NUL 结尾的 UTF-8。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RawDisplayInfo {
    pub edid_uuid: [c_char; 64],
    pub vendor: [c_char; 16],
    pub model: [c_char; 64],
    pub serial: [c_char; 64],
}

impl RawDisplayInfo {
    pub const EMPTY: Self = Self {
        edid_uuid: [0; 64],
        vendor: [0; 16],
        model: [0; 64],
        serial: [0; 64],
    };
}

/// 对应 `deskbaton_screen_mode_info`。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RawScreenMode {
    pub width: u32,
    pub height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: f64,
}

/// 对应 `deskbaton_send_cb`：`paths` 指向 `count` 个 UTF-8 路径，只在回调期间有效。
pub type SendCallback = unsafe extern "C" fn(paths: *const *const c_char, count: u32);

unsafe extern "C" {
    pub fn deskbaton_ddc_list(out: *mut RawDisplayInfo, capacity: u32) -> i32;
    pub fn deskbaton_ddc_write(
        display_index: u32,
        data: *const u8,
        len: u32,
        write_cycles: u32,
    ) -> i32;
    pub fn deskbaton_ddc_transact(
        display_index: u32,
        data: *const u8,
        data_len: u32,
        write_cycles: u32,
        reply_delay_ms: u32,
        out: *mut u8,
        out_len: u32,
    ) -> i32;
    pub fn deskbaton_screen_mode(display_index: u32, out: *mut RawScreenMode) -> i32;
    pub fn deskbaton_gamma_get(
        display_index: u32,
        red: *mut f32,
        green: *mut f32,
        blue: *mut f32,
        capacity: u32,
    ) -> i32;
    pub fn deskbaton_gamma_set(
        display_index: u32,
        red: *const f32,
        green: *const f32,
        blue: *const f32,
        count: u32,
    ) -> i32;
    pub fn deskbaton_gamma_restore_all();

    pub fn deskbaton_stay_awake_begin() -> i32;
    pub fn deskbaton_stay_awake_end() -> i32;
    pub fn deskbaton_wake_display() -> i32;
    pub fn deskbaton_watch_wake(cb: Option<unsafe extern "C" fn()>) -> i32;

    pub fn deskbaton_run_app_loop();
    pub fn deskbaton_notify(message: *const c_char) -> i32;
    pub fn deskbaton_notify_reveal(message: *const c_char, path: *const c_char) -> i32;
    pub fn deskbaton_register_send_service(cb: Option<SendCallback>) -> i32;

    pub fn deskbaton_login_item_status() -> i32;
    pub fn deskbaton_login_item_set(enabled: i32) -> i32;
    pub fn deskbaton_helper_register() -> i32;
    pub fn deskbaton_helper_set_sleep_disabled(disabled: i32) -> i32;
}

/// `kCGEventSourceStateHIDSystemState`：汇总所有硬件输入设备。
pub const CG_EVENT_SOURCE_STATE_HID_SYSTEM: i32 = 1;
/// `kCGAnyInputEventType`。
pub const CG_ANY_INPUT_EVENT_TYPE: u32 = u32::MAX;

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// 距离该来源最近一次指定类型事件的秒数。
    pub fn CGEventSourceSecondsSinceLastEventType(state_id: i32, event_type: u32) -> f64;
}
