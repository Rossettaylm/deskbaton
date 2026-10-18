//! 面板相对托盘图标的摆放计算，坐标均为同一屏幕的逻辑像素。

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Bounds {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        (self.x..self.right()).contains(&x) && (self.y..self.bottom()).contains(&y)
    }

    fn right(&self) -> f64 {
        self.x + self.width
    }

    fn bottom(&self) -> f64 {
        self.y + self.height
    }
}

/// 托盘点落在哪块屏上；都不包含时退到第一块。没有屏幕则 `None`。
pub fn screen_for_point(x: f64, y: f64, screens: &[Bounds]) -> Option<Bounds> {
    screens
        .iter()
        .copied()
        .find(|screen| screen.contains(x, y))
        .or_else(|| screens.first().copied())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAlign {
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    CenterOnTray,
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    WorkAreaEnd,
}

/// 面板相对托盘图标摆放并限制在屏幕内。`CenterOnTray` 水平居中托盘；`WorkAreaEnd` 水平贴工作区右缘。
/// 垂直：`CenterOnTray` 在图标下方放得下则放下方，否则上方；`WorkAreaEnd` 始终在图标上方（钉底边）。
/// 返回面板左上角。
pub fn panel_origin(
    tray: Bounds,
    panel: (f64, f64),
    screen: Bounds,
    gap: f64,
    align: PanelAlign,
) -> (f64, f64) {
    let (width, height) = panel;
    let x = match align {
        PanelAlign::CenterOnTray => (tray.x + tray.width / 2.0 - width / 2.0)
            .min(screen.right() - width - gap)
            .max(screen.x + gap),
        PanelAlign::WorkAreaEnd => (screen.right() - width - gap).max(screen.x + gap),
    };
    let y = match align {
        PanelAlign::CenterOnTray => {
            let below = tray.bottom() + gap;
            if below + height <= screen.bottom() {
                below
            } else {
                (tray.y - height - gap).max(screen.y)
            }
        }
        PanelAlign::WorkAreaEnd => (tray.y - height - gap).max(screen.y),
    };
    (x, y)
}

/// Windows 读不到托盘矩形时，用屏幕右下一块 36×24 充当托盘。
#[cfg_attr(not(test), allow(dead_code))]
pub fn work_area_end_tray(screen: Bounds) -> Bounds {
    Bounds {
        x: screen.x + screen.width - 36.0,
        y: screen.y + screen.height - 24.0,
        width: 36.0,
        height: 24.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Bounds = Bounds {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    fn tray(x: f64, y: f64) -> Bounds {
        Bounds {
            x,
            y,
            width: 36.0,
            height: 24.0,
        }
    }

    #[test]
    fn below_menu_bar_icon_centered() {
        assert_eq!(
            panel_origin(
                tray(900.0, 0.0),
                (360.0, 440.0),
                SCREEN,
                6.0,
                PanelAlign::CenterOnTray,
            ),
            (738.0, 30.0)
        );
    }

    #[test]
    fn kept_inside_right_edge() {
        let (x, _) = panel_origin(
            tray(1870.0, 0.0),
            (360.0, 440.0),
            SCREEN,
            6.0,
            PanelAlign::CenterOnTray,
        );
        assert_eq!(x, 1920.0 - 360.0 - 6.0);
    }

    #[test]
    fn above_bottom_taskbar_icon() {
        assert_eq!(
            panel_origin(
                tray(1700.0, 1048.0),
                (360.0, 440.0),
                SCREEN,
                6.0,
                PanelAlign::CenterOnTray,
            ),
            (1538.0, 602.0)
        );
    }

    #[test]
    fn contains_is_half_open() {
        assert!(SCREEN.contains(0.0, 0.0));
        assert!(SCREEN.contains(1919.5, 1079.5));
        assert!(!SCREEN.contains(1920.0, 10.0));
        assert!(!SCREEN.contains(10.0, -1.0));
    }

    #[test]
    fn respects_screen_offset_on_secondary_display() {
        let screen = Bounds {
            x: 1920.0,
            y: -515.0,
            width: 1080.0,
            height: 1920.0,
        };
        assert_eq!(
            panel_origin(
                tray(1925.0, -515.0),
                (360.0, 440.0),
                screen,
                6.0,
                PanelAlign::CenterOnTray,
            ),
            (1926.0, -485.0)
        );
    }

    fn secondary() -> Bounds {
        Bounds {
            x: 1920.0,
            y: -515.0,
            width: 1080.0,
            height: 1920.0,
        }
    }

    #[test]
    fn tray_point_picks_the_screen_that_contains_it() {
        let screens = [SCREEN, secondary()];
        assert_eq!(screen_for_point(100.0, 10.0, &screens), Some(SCREEN));
        assert_eq!(
            screen_for_point(1925.0, -515.0, &screens),
            Some(secondary())
        );
    }

    #[test]
    fn tray_point_outside_all_screens_falls_back_to_first() {
        let screens = [SCREEN, secondary()];
        assert_eq!(screen_for_point(-80.0, -80.0, &screens), Some(SCREEN));
    }

    #[test]
    fn tray_point_with_no_screens_is_none() {
        assert_eq!(screen_for_point(0.0, 0.0, &[]), None);
    }

    #[test]
    fn work_area_end_pins_bottom_right() {
        assert_eq!(
            panel_origin(
                tray(1700.0, 1048.0),
                (360.0, 440.0),
                SCREEN,
                12.0,
                PanelAlign::WorkAreaEnd,
            ),
            (1548.0, 596.0)
        );
    }

    #[test]
    fn work_area_end_clamps_wide_panel() {
        let (x, _) = panel_origin(
            tray(1700.0, 1048.0),
            (2000.0, 440.0),
            SCREEN,
            12.0,
            PanelAlign::WorkAreaEnd,
        );
        assert_eq!(x, 12.0);
    }

    #[test]
    fn work_area_end_clamps_y_to_screen() {
        let (_, y) = panel_origin(
            tray(1700.0, 100.0),
            (360.0, 440.0),
            SCREEN,
            12.0,
            PanelAlign::WorkAreaEnd,
        );
        assert_eq!(y, 0.0);
    }

    #[test]
    fn work_area_end_tray_sits_at_bottom_right() {
        assert_eq!(
            work_area_end_tray(SCREEN),
            Bounds {
                x: 1884.0,
                y: 1056.0,
                width: 36.0,
                height: 24.0,
            }
        );
    }
}
