//! 全局热键。macOS / Windows 需要在主线程运行系统事件循环，热键事件才会被分发：
//! 无界面程序用 [`run_main_loop`]，面板由 Tauri 的事件循环分发，只需 [`register`]。

use std::sync::mpsc::Sender;

use baton_core::Result;
use baton_core::event::Event;

pub use imp::{Hotkey, Registration, parse};

/// 在主线程注册热键，按下时发送 [`Event::Hotkey`]；返回值被丢弃时注销。
pub fn register(hotkey: Hotkey, events: Sender<Event>) -> Result<Registration> {
    imp::register(hotkey, events)
}

/// 注册热键并在当前（主）线程运行事件循环。
pub fn run_main_loop(hotkey: Hotkey, events: Sender<Event>) -> Result<()> {
    let registration = register(hotkey, events)?;
    imp::platform_loop();
    drop(registration);
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use std::sync::mpsc::Sender;

    use baton_core::event::Event;
    use baton_core::{Error, Result};
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

    pub type Hotkey = HotKey;

    pub struct Registration {
        _manager: GlobalHotKeyManager,
    }

    pub fn parse(text: &str) -> Result<Hotkey> {
        text.parse()
            .map_err(|e| Error::Config(format!("无法解析热键「{text}」：{e}")))
    }

    pub fn register(hotkey: Hotkey, events: Sender<Event>) -> Result<Registration> {
        let manager = GlobalHotKeyManager::new()
            .map_err(|e| Error::Input(format!("无法初始化全局热键：{e}")))?;
        manager
            .register(hotkey)
            .map_err(|e| Error::Input(format!("注册热键失败（可能已被其他程序占用）：{e}")))?;
        let id = hotkey.id();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.id == id && event.state == HotKeyState::Pressed {
                let _ = events.send(Event::Hotkey);
            }
        }));
        Ok(Registration { _manager: manager })
    }

    #[cfg(target_os = "macos")]
    pub fn platform_loop() {
        // SAFETY: 无参数；main 在主线程调用，满足 NSApplication 的线程要求。
        unsafe { baton_macos_sys::deskbaton_run_app_loop() };
    }

    #[cfg(target_os = "windows")]
    pub fn platform_loop() {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, GetMessageW, MSG, TranslateMessage,
        };
        let mut msg = MSG::default();
        // SAFETY: 标准消息循环，`msg` 是局部变量；热键窗口由 GlobalHotKeyManager 在本线程创建。
        unsafe {
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use std::sync::mpsc::Sender;

    use baton_core::Result;
    use baton_core::event::Event;

    pub struct Hotkey;

    pub struct Registration;

    pub fn parse(_: &str) -> Result<Hotkey> {
        Ok(Hotkey)
    }

    pub fn register(_: Hotkey, _: Sender<Event>) -> Result<Registration> {
        log::warn!("当前平台不支持全局热键，请使用 --stdin 触发交棒");
        Ok(Registration)
    }

    pub fn platform_loop() {
        loop {
            std::thread::park();
        }
    }
}
