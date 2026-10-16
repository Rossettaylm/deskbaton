//! 显示器输入源控制（DDC/CI）的各平台实现。

#[cfg(any(target_os = "macos", test))]
mod mccs;
mod quirks;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use quirks::InputSwitchMethod;

use baton_core::ports::DisplayControl;

pub fn platform_display() -> Box<dyn DisplayControl> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacDdc);
    #[cfg(target_os = "windows")]
    return Box::new(windows::WinDdc);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(unsupported::UnsupportedDisplay);
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use baton_core::ids::{DisplayId, InputCode};
    use baton_core::ports::{DisplayControl, DisplayInfo};
    use baton_core::{Error, Result};

    pub struct UnsupportedDisplay;

    const WHAT: &str = "当前操作系统的 DDC/CI";

    impl DisplayControl for UnsupportedDisplay {
        fn list(&self) -> Result<Vec<DisplayInfo>> {
            Err(Error::Unsupported(WHAT))
        }
        fn get_input(&self, _: &DisplayId) -> Result<InputCode> {
            Err(Error::Unsupported(WHAT))
        }
        fn set_input(&self, _: &DisplayId, _: InputCode) -> Result<()> {
            Err(Error::Unsupported(WHAT))
        }
    }
}
