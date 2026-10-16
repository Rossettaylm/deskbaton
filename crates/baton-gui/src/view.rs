//! 面板展示用的数据组装，不依赖 Tauri，便于以后并入 agent 的托盘。

use baton_core::config::InputPreset;
use baton_core::ids::InputCode;
use baton_core::ports::DisplayInfo;
use baton_display::ScreenMode;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DisplayView {
    pub id: String,
    pub name: String,
    pub serial: Option<String>,
    pub subtitle: String,
    pub screen: Option<ScreenView>,
    /// 读不到时为 `None`，通常是本机不是显示器当前的活跃输入。
    pub current_input: Option<u16>,
    pub current_label: Option<String>,
    pub inputs: Vec<InputButton>,
    /// `display.match` 命中的那台，作为默认选中。
    pub preferred: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScreenView {
    pub width: u32,
    pub height: u32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub refresh_hz: Option<u32>,
    pub hidpi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputButton {
    pub label: String,
    pub code: u16,
    pub kind: PortKind,
    pub active: bool,
}

/// 决定按钮上画哪种接口轮廓。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortKind {
    Dp,
    Hdmi,
    UsbC,
    Other,
}

impl PortKind {
    fn from_label(label: &str) -> Self {
        let label = label.to_ascii_uppercase();
        if label.contains("HDMI") {
            Self::Hdmi
        } else if label.contains("USB") || label.contains("TYPE-C") || label.contains("TYPEC") {
            Self::UsbC
        } else if label.contains("DP") || label.contains("DISPLAYPORT") {
            Self::Dp
        } else {
            Self::Other
        }
    }
}

pub fn display_view(
    info: &DisplayInfo,
    screen: Option<ScreenMode>,
    current: Option<InputCode>,
    presets: &[InputPreset],
    preferred: bool,
) -> DisplayView {
    let screen = screen.map(screen_view);
    let current_label = current.map(|code| input_label(presets, code));
    DisplayView {
        id: info.id.0.clone(),
        name: display_name(info),
        serial: info.serial.clone(),
        subtitle: subtitle(current_label.as_deref(), screen.as_ref()),
        screen,
        current_input: current.map(|c| c.0),
        current_label,
        inputs: input_buttons(presets, current),
        preferred,
    }
}

/// 当前编码不在配置里时追加一个以编码命名的按钮，保证当前输入源总能高亮。
pub fn input_buttons(presets: &[InputPreset], current: Option<InputCode>) -> Vec<InputButton> {
    let mut buttons: Vec<InputButton> = presets
        .iter()
        .map(|p| InputButton {
            label: p.label.clone(),
            code: p.code.0,
            kind: PortKind::from_label(&p.label),
            active: current == Some(p.code),
        })
        .collect();
    if let Some(code) = current
        && !presets.iter().any(|p| p.code == code)
    {
        buttons.push(InputButton {
            label: hex(code),
            code: code.0,
            kind: PortKind::Other,
            active: true,
        });
    }
    buttons
}

pub fn input_label(presets: &[InputPreset], code: InputCode) -> String {
    presets
        .iter()
        .find(|p| p.code == code)
        .map_or_else(|| hex(code), |p| p.label.clone())
}

fn hex(code: InputCode) -> String {
    format!("0x{:02X}", code.0)
}

/// EDID 厂商码（如 HWP）对用户没有意义，型号里通常已带品牌，所以只用型号。
fn display_name(info: &DisplayInfo) -> String {
    if info.model.is_empty() {
        "外接显示器".to_owned()
    } else {
        info.model.clone()
    }
}

fn screen_view(mode: ScreenMode) -> ScreenView {
    ScreenView {
        width: mode.width,
        height: mode.height,
        pixel_width: mode.pixel_width,
        pixel_height: mode.pixel_height,
        refresh_hz: mode.refresh_hz.map(|hz| hz.round() as u32),
        hidpi: mode.is_hidpi(),
    }
}

/// 形如「外接显示器 · DP · 2560×1440 · 60Hz」，缺失的部分省略。
pub fn subtitle(current_label: Option<&str>, screen: Option<&ScreenView>) -> String {
    let mut parts = vec!["外接显示器".to_owned()];
    parts.extend(current_label.map(str::to_owned));
    if let Some(s) = screen {
        parts.push(format!("{}×{}", s.width, s.height));
        parts.extend(s.refresh_hz.map(|hz| format!("{hz}Hz")));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use baton_core::ids::DisplayId;

    use super::*;

    fn presets() -> Vec<InputPreset> {
        vec![
            InputPreset::new("DP", 0x0F),
            InputPreset::new("HDMI1", 0x11),
            InputPreset::new("USB-C", 0x1B),
        ]
    }

    fn mode() -> ScreenMode {
        ScreenMode {
            width: 2560,
            height: 1440,
            pixel_width: 5120,
            pixel_height: 2880,
            refresh_hz: Some(59.94),
        }
    }

    fn info() -> DisplayInfo {
        DisplayInfo {
            id: DisplayId("UUID-1".into()),
            vendor: "HWP".into(),
            model: "HP D27k".into(),
            serial: Some("BVTB42350197".into()),
        }
    }

    #[test]
    fn highlights_configured_current_input() {
        let buttons = input_buttons(&presets(), Some(InputCode(0x11)));
        let active: Vec<&str> = buttons
            .iter()
            .filter(|b| b.active)
            .map(|b| b.label.as_str())
            .collect();
        assert_eq!(active, ["HDMI1"]);
        assert_eq!(buttons.len(), 3);
    }

    #[test]
    fn appends_unknown_current_input_as_hex() {
        let buttons = input_buttons(&presets(), Some(InputCode(0x12)));
        let last = buttons.last().unwrap();
        assert_eq!(buttons.len(), 4);
        assert_eq!(
            (last.label.as_str(), last.code, last.kind, last.active),
            ("0x12", 0x12, PortKind::Other, true)
        );
        assert_eq!(input_label(&presets(), InputCode(0x12)), "0x12");
    }

    #[test]
    fn nothing_active_when_current_input_unreadable() {
        let buttons = input_buttons(&presets(), None);
        assert!(buttons.iter().all(|b| !b.active));
        assert_eq!(buttons.len(), 3);
    }

    #[test]
    fn port_kind_follows_label() {
        let kinds: Vec<PortKind> = ["DP", "HDMI2", "USB-C", "Type-C", "DisplayPort 2", "VGA"]
            .iter()
            .map(|l| PortKind::from_label(l))
            .collect();
        assert_eq!(
            kinds,
            [
                PortKind::Dp,
                PortKind::Hdmi,
                PortKind::UsbC,
                PortKind::UsbC,
                PortKind::Dp,
                PortKind::Other
            ]
        );
    }

    #[test]
    fn subtitle_includes_input_resolution_and_refresh() {
        let view = display_view(
            &info(),
            Some(mode()),
            Some(InputCode(0x0F)),
            &presets(),
            true,
        );
        assert_eq!(view.subtitle, "外接显示器 · DP · 2560×1440 · 60Hz");
        assert_eq!(view.name, "HP D27k");
        assert!(view.screen.as_ref().unwrap().hidpi);
        assert_eq!(view.current_label.as_deref(), Some("DP"));
    }

    #[test]
    fn subtitle_omits_missing_parts() {
        let view = display_view(&info(), None, None, &presets(), false);
        assert_eq!(view.subtitle, "外接显示器");
        let no_refresh = ScreenMode {
            refresh_hz: None,
            pixel_width: 2560,
            pixel_height: 1440,
            ..mode()
        };
        let view = display_view(&info(), Some(no_refresh), None, &presets(), false);
        assert_eq!(view.subtitle, "外接显示器 · 2560×1440");
        assert!(!view.screen.unwrap().hidpi);
    }
}
