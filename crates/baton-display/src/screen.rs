use baton_core::Result;
use baton_core::ids::DisplayId;

/// 显示器当前的显示模式。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenMode {
    /// 逻辑分辨率。
    pub width: u32,
    pub height: u32,
    /// 渲染分辨率；大于逻辑分辨率即为 HiDPI。
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: Option<f64>,
}

impl ScreenMode {
    pub fn is_hidpi(&self) -> bool {
        self.pixel_width > self.width
    }
}

/// `display` 为 [`crate::platform_display`] 列出的 id。
pub fn screen_mode(display: &DisplayId) -> Result<ScreenMode> {
    #[cfg(target_os = "macos")]
    return crate::macos::screen_mode(display);
    #[cfg(target_os = "windows")]
    return crate::windows::screen_mode(display);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = display;
        Err(baton_core::Error::Unsupported("当前操作系统的显示模式查询"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidpi_when_rendered_above_logical_size() {
        let mode = |pixel_width| ScreenMode {
            width: 2560,
            height: 1440,
            pixel_width,
            pixel_height: 1440,
            refresh_hz: Some(60.0),
        };
        assert!(mode(5120).is_hidpi());
        assert!(!mode(2560).is_hidpi());
    }
}
