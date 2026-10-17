//! 面板开机自启：macOS LaunchAgent，Windows 当前用户 Run 键。

use std::path::PathBuf;

use baton_core::{Error, Result};

#[cfg(target_os = "macos")]
const LABEL: &str = "com.deskbaton.gui";
#[cfg(target_os = "windows")]
const RUN_VALUE: &str = "DeskBaton";

pub fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().map_err(|e| Error::Config(format!("无法确定本程序路径：{e}")))
}

pub fn is_enabled() -> Result<bool> {
    imp::is_enabled()
}

pub fn set_enabled(on: bool) -> Result<()> {
    if on {
        imp::enable(&current_exe()?)
    } else {
        imp::disable()
    }
}

/// 登录项已开启时把目标改成当前可执行文件。
pub fn refresh_path() -> Result<()> {
    imp::enable(&current_exe()?)
}

#[cfg(target_os = "macos")]
mod imp {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{LABEL, Result};
    use baton_core::Error;

    pub fn is_enabled() -> Result<bool> {
        Ok(plist_path()?.is_file())
    }

    pub fn enable(exe: &Path) -> Result<()> {
        let path = plist_path()?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| Error::Config(format!("无法创建登录项目录：{e}")))?;
        }
        fs::write(&path, plist_body(exe))
            .map_err(|e| Error::Config(format!("无法写入登录项：{e}")))?;
        let domain = gui_domain()?;
        let id = format!("{domain}/{LABEL}");
        let _ = Command::new("launchctl").args(["bootout", &id]).status();
        let status = Command::new("launchctl")
            .args(["bootstrap", &domain, path.to_str().unwrap_or_default()])
            .status()
            .map_err(|e| Error::Config(format!("无法登记登录项：{e}")))?;
        if !status.success() {
            return Err(Error::Config("无法登记登录项".into()));
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let domain = gui_domain()?;
        let id = format!("{domain}/{LABEL}");
        let _ = Command::new("launchctl").args(["bootout", &id]).status();
        let path = plist_path()?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Config(format!("无法删除登录项：{e}"))),
        }
    }

    fn plist_path() -> Result<PathBuf> {
        let home =
            std::env::var_os("HOME").ok_or_else(|| Error::Config("环境变量 HOME 未设置".into()))?;
        Ok(PathBuf::from(home)
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    fn gui_domain() -> Result<String> {
        let output = Command::new("id")
            .arg("-u")
            .output()
            .map_err(|e| Error::Config(format!("无法读取用户 ID：{e}")))?;
        if !output.status.success() {
            return Err(Error::Config("无法读取用户 ID".into()));
        }
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok(format!("gui/{uid}"))
    }

    fn plist_body(exe: &Path) -> String {
        let path = exe
            .to_string_lossy()
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{path}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
        )
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use super::{RUN_VALUE, Result};
    use baton_core::Error;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_SZ, RegCloseKey, RegDeleteValueW,
        RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    };
    use windows::core::{PCWSTR, w};

    const RUN_PATH: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

    pub fn is_enabled() -> Result<bool> {
        let key = open(KEY_READ)?;
        let name = wide(RUN_VALUE);
        let mut kind = REG_SZ;
        let mut size = 0u32;
        // SAFETY: `key` 已打开，`name` 以 NUL 结尾；第一次只问长度。
        let status = unsafe {
            RegQueryValueExW(
                key,
                PCWSTR(name.as_ptr()),
                None,
                Some(&mut kind),
                None,
                Some(&mut size),
            )
        };
        close(key);
        Ok(status == ERROR_SUCCESS)
    }

    pub fn enable(exe: &Path) -> Result<()> {
        let key = open(KEY_READ | KEY_SET_VALUE)?;
        let name = wide(RUN_VALUE);
        let value = wide(&quoted(exe));
        let bytes =
            unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
        // SAFETY: `key` 已打开；`name` / `value` 以 NUL 结尾，字节长度与 UTF-16 缓冲一致。
        let status = unsafe { RegSetValueExW(key, PCWSTR(name.as_ptr()), 0, REG_SZ, Some(bytes)) };
        close(key);
        if status != ERROR_SUCCESS {
            return Err(Error::Config("无法写入开机启动项".into()));
        }
        Ok(())
    }

    pub fn disable() -> Result<()> {
        let key = open(KEY_SET_VALUE)?;
        let name = wide(RUN_VALUE);
        // SAFETY: `key` 已打开，`name` 以 NUL 结尾。
        let status = unsafe { RegDeleteValueW(key, PCWSTR(name.as_ptr())) };
        close(key);
        if status == ERROR_SUCCESS
            || status
                == windows::Win32::Foundation::WIN32_ERROR(
                    windows::Win32::Foundation::ERROR_FILE_NOT_FOUND.0,
                )
        {
            return Ok(());
        }
        Err(Error::Config("无法删除开机启动项".into()))
    }

    fn quoted(exe: &Path) -> String {
        let path = exe.to_string_lossy();
        if path.contains(' ') {
            format!("\"{path}\"")
        } else {
            path.into_owned()
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn open(
        access: windows::Win32::System::Registry::REG_SAM_FLAGS,
    ) -> Result<windows::Win32::System::Registry::HKEY> {
        let mut key = windows::Win32::System::Registry::HKEY::default();
        // SAFETY: `RUN_PATH` 是静态宽字符串；`key` 由 API 写入。
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, RUN_PATH, 0, access, &mut key) };
        if status != ERROR_SUCCESS {
            return Err(Error::Config("无法打开开机启动注册表项".into()));
        }
        Ok(key)
    }

    fn close(key: windows::Win32::System::Registry::HKEY) {
        // SAFETY: `key` 由 `open` 取得，只关闭一次。
        let _ = unsafe { RegCloseKey(key) };
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use std::path::Path;

    use super::Result;
    use baton_core::Error;

    pub fn is_enabled() -> Result<bool> {
        Err(Error::Unsupported("开机自启"))
    }

    pub fn enable(_: &Path) -> Result<()> {
        Err(Error::Unsupported("开机自启"))
    }

    pub fn disable() -> Result<()> {
        Err(Error::Unsupported("开机自启"))
    }
}
