//! 显示器控制（DDC/CI）的各平台实现：输入源、屏幕模式、亮度与音量。

#[cfg(any(target_os = "windows", test))]
mod edid;
mod levels;
#[cfg(any(target_os = "macos", test))]
mod mccs;
mod quirks;
mod screen;
mod vcp;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

pub use levels::{Control, Levels, LevelsView, Slider, Source};
pub use quirks::InputSwitchMethod;
pub use screen::{ScreenMode, screen_mode};

use baton_core::ids::DisplayId;
use baton_core::ports::DisplayControl;

pub fn platform_display() -> Box<dyn DisplayControl> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacDdc);
    #[cfg(target_os = "windows")]
    return Box::new(windows::WinDdc);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(unsupported::UnsupportedDisplay);
}

fn platform_levels_backend() -> Box<dyn levels::Backend> {
    #[cfg(target_os = "macos")]
    return Box::new(macos::MacLevels);
    #[cfg(target_os = "windows")]
    return Box::new(windows::WinLevels);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Box::new(unsupported::UnsupportedLevels);
}

/// 复原上次异常退出时残留的软件调光。macOS 会把所有显示器恢复为 ColorSync 描述文件，所以只在有残留时调用。
pub fn restore_dimmed(displays: &[DisplayId]) {
    #[cfg(target_os = "macos")]
    if !displays.is_empty() {
        macos::restore_all_gamma();
    }
    #[cfg(target_os = "windows")]
    for display in displays {
        windows::reset_gamma(display);
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let _ = displays;
}

/// DDC 读写失败时共尝试 3 次，间隔 200 毫秒。
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn retry<T>(mut op: impl FnMut() -> baton_core::Result<T>) -> baton_core::Result<T> {
    const ATTEMPTS: usize = 3;
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(200);
    let mut last = op();
    for _ in 1..ATTEMPTS {
        if last.is_ok() {
            break;
        }
        std::thread::sleep(INTERVAL);
        last = op();
    }
    last
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod unsupported {
    use baton_core::ids::{DisplayId, InputCode};
    use baton_core::ports::{DisplayControl, DisplayInfo};
    use baton_core::{Error, Result};

    use crate::levels::{Backend, DimCurve, EndpointVolume, GammaTable};
    use crate::vcp::VcpRead;

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

    pub struct UnsupportedLevels;

    impl Backend for UnsupportedLevels {
        fn get_vcp(&self, _: &DisplayId, _: u8) -> Result<VcpRead> {
            Err(Error::Unsupported(WHAT))
        }
        fn set_vcp(&self, _: &DisplayId, _: u8, _: u16) -> Result<()> {
            Err(Error::Unsupported(WHAT))
        }
        fn gamma(&self, _: &DisplayId) -> Option<GammaTable> {
            None
        }
        fn set_gamma(&self, _: &DisplayId, _: &GammaTable) -> Result<()> {
            Err(Error::Unsupported("当前操作系统的软件调光"))
        }
        fn dim_curve(&self) -> DimCurve {
            DimCurve::Linear { floor: 0.15 }
        }
        fn endpoint(&self, _: &str) -> Result<Option<EndpointVolume>> {
            Ok(None)
        }
        fn set_endpoint_level(&self, _: &str, _: f32) -> Result<()> {
            Err(Error::Unsupported("当前操作系统的系统音量"))
        }
        fn set_endpoint_mute(&self, _: &str, _: bool) -> Result<()> {
            Err(Error::Unsupported("当前操作系统的系统音量"))
        }
    }
}
