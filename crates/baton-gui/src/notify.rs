//! 系统通知。失败只记日志，不打断交棒。

pub fn show(message: &str) {
    if let Err(e) = imp::show(message) {
        log::info!("系统通知不可用：{e}");
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::CString;

    use baton_core::{Error, Result};
    use baton_macos_sys::{self as sys, deskbaton_notify};

    pub fn show(message: &str) -> Result<()> {
        let cstr = CString::new(message).map_err(|_| Error::Config("通知文案含有空字节".into()))?;
        // SAFETY: `cstr` 在调用期间保持有效，原生侧只读取。
        let rc = unsafe { deskbaton_notify(cstr.as_ptr()) };
        if rc == sys::OK {
            Ok(())
        } else {
            Err(Error::Config("无法弹出系统通知".into()))
        }
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::OnceLock;

    use baton_core::{Error, Result};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Shell::{
        NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_MODIFY, NOTIFYICONDATAW,
        Shell_NotifyIconW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, HWND_MESSAGE, RegisterClassW, WINDOW_EX_STYLE,
        WINDOW_STYLE, WM_USER, WNDCLASSW,
    };
    use windows::core::{PCWSTR, w};

    const CALLBACK: u32 = WM_USER + 1;

    static WINDOW: OnceLock<isize> = OnceLock::new();

    pub fn show(message: &str) -> Result<()> {
        let raw = *WINDOW.get_or_init(message_window);
        if raw == 0 {
            return Err(Error::Config("无法创建通知窗口".into()));
        }
        // SAFETY: `raw` 来自本进程 `CreateWindowExW` 返回的窗口句柄。
        let hwnd = HWND(raw as *mut _);
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_INFO | NIF_TIP | NIF_MESSAGE | NIF_ICON,
            uCallbackMessage: CALLBACK,
            dwInfoFlags: NIIF_INFO,
            ..Default::default()
        };
        copy_wide(&mut data.szTip, "DeskBaton");
        copy_wide(&mut data.szInfoTitle, "DeskBaton");
        copy_wide(&mut data.szInfo, message);
        // SAFETY: `data` 字段已填满，窗口在本进程内。
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &data);
            let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
        }
        Ok(())
    }

    fn copy_wide(dest: &mut [u16], text: &str) {
        dest.fill(0);
        let limit = dest.len().saturating_sub(1);
        for (slot, unit) in dest.iter_mut().zip(text.encode_utf16().take(limit)) {
            *slot = unit;
        }
    }

    fn message_window() -> isize {
        let class = w!("DeskBatonNotify");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            lpszClassName: class,
            ..Default::default()
        };
        // SAFETY: 标准消息窗口注册；失败时仍尝试创建。
        unsafe {
            let _ = RegisterClassW(&wc);
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!("DeskBaton"),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
            .map(|hwnd| hwnd.0 as isize)
            .unwrap_or(0)
        }
    }

    // SAFETY: 消息窗口的默认过程，不使用额外用户数据。
    unsafe extern "system" fn wndproc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use baton_core::{Error, Result};

    pub fn show(_: &str) -> Result<()> {
        Err(Error::Unsupported("系统通知"))
    }
}
