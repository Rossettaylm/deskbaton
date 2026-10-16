//! macOS 原生胶水的 FFI 声明，对应 `native/deskbaton_native.h`。
//! 返回值约定：`0` 成功，负数为错误码。

#![cfg(target_os = "macos")]

pub const OK: i32 = 0;
pub const ERR_UNIMPLEMENTED: i32 = -1;

unsafe extern "C" {
    pub fn deskbaton_ddc_display_count() -> i32;
    pub fn deskbaton_ddc_get_input(display_index: u32, out_input: *mut u16) -> i32;
    pub fn deskbaton_ddc_set_input(display_index: u32, input: u16) -> i32;

    pub fn deskbaton_stay_awake_begin(seconds: u32) -> i32;
    pub fn deskbaton_stay_awake_end() -> i32;
    pub fn deskbaton_wake_display() -> i32;
}
