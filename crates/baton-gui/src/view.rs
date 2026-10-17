//! 面板展示用的数据组装，不依赖 Tauri，便于以后并入 agent 的托盘。

use baton_core::config::InputPreset;
use baton_core::ids::{InputCode, Platform};
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
    /// 交棒显示器上按电脑列出的交棒目标；其它显示器或交棒服务未启动时为 `None`，显示接口按钮。
    pub computers: Option<Vec<ComputerView>>,
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
    pub scale_percent: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InputButton {
    pub label: String,
    pub code: u16,
    pub kind: PortKind,
    pub active: bool,
    /// 接在这个口上的电脑；显示器把两个口报成同一编码时会有多台。
    pub owners: Vec<PortOwner>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortOwner {
    pub name: String,
    pub platform: Option<Platform>,
    pub local: bool,
}

/// 「电脑」卡片上的一格。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ComputerView {
    pub name: String,
    pub platform: Option<Platform>,
    pub local: bool,
    pub online: bool,
    /// 接口名；还没识别时为 `None`。
    pub port: Option<String>,
    /// 显示器当前显示的就是它。
    pub active: bool,
}

/// 已配对的设备。
#[derive(Debug, Clone)]
pub struct PeerMachine {
    pub name: String,
    pub platform: Option<Platform>,
    pub code: Option<InputCode>,
    pub online: bool,
    /// 心跳报告它持棒。
    pub holding: bool,
}

/// 面板知道的电脑：本机与已配对设备。
#[derive(Debug, Clone, Default)]
pub struct Machines {
    /// 本机名称；交棒服务未启动时为空。
    pub me: Option<String>,
    /// 本机输入源编码（配置值或自动学到的值），只对 `display.match` 命中的显示器有效。
    pub my_code: Option<InputCode>,
    pub peers: Vec<PeerMachine>,
}

impl Machines {
    /// 本机在前，其余按名称排序；交棒服务未启动时为 `None`。
    /// 读得到显示器时按当前编码高亮；读不到（本机不持棒）时按对端心跳报告的持棒状态高亮。
    fn computers(
        &self,
        presets: &[InputPreset],
        current: Option<InputCode>,
    ) -> Option<Vec<ComputerView>> {
        let me = self.me.as_ref()?;
        let view =
            |name: &str, platform, local, online, code: Option<InputCode>, holding| ComputerView {
                name: name.to_owned(),
                platform,
                local,
                online,
                port: code.map(|c| port_name(presets, c)),
                active: match current {
                    Some(current) => code == Some(current),
                    None => holding,
                },
            };
        let mut peers: Vec<&PeerMachine> = self.peers.iter().collect();
        peers.sort_by(|a, b| a.name.cmp(&b.name));
        let local = view(
            me,
            Some(Platform::current()),
            true,
            true,
            self.my_code,
            false,
        );
        Some(
            std::iter::once(local)
                .chain(
                    peers
                        .into_iter()
                        .map(|p| view(&p.name, p.platform, false, p.online, p.code, p.holding)),
                )
                .collect(),
        )
    }

    /// 显示器只对活跃输入口回读请求，所以读得到的编码就是本机在用的口。
    fn on_display(
        &self,
        current: Option<InputCode>,
        preferred: bool,
    ) -> Vec<(InputCode, PortOwner)> {
        let my_code = if preferred {
            self.my_code.or(current)
        } else {
            current
        };
        let me = my_code.map(|code| {
            let owner = PortOwner {
                name: self.me.clone().unwrap_or_else(|| "本机".to_owned()),
                platform: Some(Platform::current()),
                local: true,
            };
            (code, owner)
        });
        let peers = self.peers.iter().filter_map(|p| {
            let owner = PortOwner {
                name: p.name.clone(),
                platform: p.platform,
                local: false,
            };
            Some((p.code?, owner))
        });
        me.into_iter().chain(peers).collect()
    }
}

/// 配置里的接口名优先，其次 MCCS 常见值，都不是时写编码。
pub fn port_name(presets: &[InputPreset], code: InputCode) -> String {
    if let Some(preset) = presets.iter().find(|p| p.code == code) {
        return preset.label.clone();
    }
    let common = match code.0 {
        0x01 => "VGA-1",
        0x03 => "DVI-1",
        0x04 => "DVI-2",
        0x0F => "DP-1",
        0x10 => "DP-2",
        0x11 => "HDMI-1",
        0x12 => "HDMI-2",
        0x1B => "USB-C",
        _ => return hex(code),
    };
    common.to_owned()
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
    machines: &Machines,
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
        inputs: input_buttons(presets, current, &machines.on_display(current, preferred)),
        computers: preferred
            .then(|| machines.computers(presets, current))
            .flatten(),
        preferred,
    }
}

/// 当前编码或有电脑占用的编码不在配置里时，追加以编码命名的按钮，保证它们都有地方显示。
pub fn input_buttons(
    presets: &[InputPreset],
    current: Option<InputCode>,
    machines: &[(InputCode, PortOwner)],
) -> Vec<InputButton> {
    let mut extra: Vec<InputCode> = Vec::new();
    for code in current.into_iter().chain(machines.iter().map(|(c, _)| *c)) {
        if !presets.iter().any(|p| p.code == code) && !extra.contains(&code) {
            extra.push(code);
        }
    }
    let ports = presets
        .iter()
        .map(|p| (p.label.clone(), p.code, PortKind::from_label(&p.label)))
        .chain(extra.into_iter().map(|c| (hex(c), c, PortKind::Other)));
    ports
        .map(|(label, code, kind)| InputButton {
            label,
            code: code.0,
            kind,
            active: current == Some(code),
            owners: machines
                .iter()
                .filter(|(c, _)| *c == code)
                .map(|(_, owner)| owner.clone())
                .collect(),
        })
        .collect()
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
        scale_percent: mode.scale_percent,
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
            scale_percent: None,
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
        let buttons = input_buttons(&presets(), Some(InputCode(0x11)), &[]);
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
        let buttons = input_buttons(&presets(), Some(InputCode(0x12)), &[]);
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
        let buttons = input_buttons(&presets(), None, &[]);
        assert!(buttons.iter().all(|b| !b.active));
        assert_eq!(buttons.len(), 3);
    }

    fn peer(name: &str, platform: Platform) -> PortOwner {
        PortOwner {
            name: name.into(),
            platform: Some(platform),
            local: false,
        }
    }

    fn peer_machine(name: &str, code: Option<u16>, online: bool) -> PeerMachine {
        PeerMachine {
            name: name.into(),
            platform: Some(Platform::Windows),
            code: code.map(InputCode),
            online,
            holding: false,
        }
    }

    fn machines(my_code: Option<u16>) -> Machines {
        Machines {
            me: Some("mbp".into()),
            my_code: my_code.map(InputCode),
            peers: vec![peer_machine("win", Some(0x11), true)],
        }
    }

    #[test]
    fn computers_list_local_first_with_port_names_and_highlight() {
        let machines = Machines {
            me: Some("mbp".into()),
            my_code: Some(InputCode(0x0F)),
            peers: vec![
                peer_machine("zz-pc", None, false),
                peer_machine("ptking-pc", Some(0x10), true),
            ],
        };
        let view = display_view(
            &info(),
            None,
            Some(InputCode(0x10)),
            &presets(),
            true,
            &machines,
        );
        let computers = view.computers.unwrap();
        let rows: Vec<(&str, bool, bool, Option<&str>, bool)> = computers
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.local,
                    c.online,
                    c.port.as_deref(),
                    c.active,
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("mbp", true, true, Some("DP"), false),
                ("ptking-pc", false, true, Some("DP-2"), true),
                ("zz-pc", false, false, None, false),
            ]
        );
        assert_eq!(computers[0].platform, Some(Platform::current()));
    }

    #[test]
    fn holding_peer_is_highlighted_only_when_display_unreadable() {
        let machines = Machines {
            me: Some("mbp".into()),
            my_code: Some(InputCode(0x0F)),
            peers: vec![PeerMachine {
                holding: true,
                ..peer_machine("win", Some(0x10), true)
            }],
        };
        let active = |current| {
            display_view(&info(), None, current, &presets(), true, &machines)
                .computers
                .unwrap()
                .into_iter()
                .filter(|c| c.active)
                .map(|c| c.name)
                .collect::<Vec<_>>()
        };
        assert_eq!(active(None), ["win"]);
        assert_eq!(active(Some(InputCode(0x0F))), ["mbp"]);
    }

    #[test]
    fn computers_only_on_preferred_display_with_agent() {
        let other = display_view(&info(), None, None, &presets(), false, &machines(None));
        assert!(other.computers.is_none());
        let no_agent = display_view(&info(), None, None, &presets(), true, &Machines::default());
        assert!(no_agent.computers.is_none());
    }

    #[test]
    fn port_names_prefer_config_then_mccs_then_hex() {
        assert_eq!(port_name(&presets(), InputCode(0x0F)), "DP");
        assert_eq!(port_name(&presets(), InputCode(0x12)), "HDMI-2");
        assert_eq!(port_name(&presets(), InputCode(0x55)), "0x55");
    }

    /// `(按钮名, 占用设备名)`，只列有设备的按钮。
    fn owned(view: &DisplayView) -> Vec<(&str, Vec<&str>)> {
        view.inputs
            .iter()
            .filter(|b| !b.owners.is_empty())
            .map(|b| {
                let names = b.owners.iter().map(|o| o.name.as_str()).collect();
                (b.label.as_str(), names)
            })
            .collect()
    }

    #[test]
    fn readable_input_is_marked_as_local() {
        let view = display_view(
            &info(),
            None,
            Some(InputCode(0x1B)),
            &presets(),
            false,
            &machines(Some(0x0F)),
        );
        assert_eq!(
            owned(&view),
            [("HDMI1", vec!["win"]), ("USB-C", vec!["mbp"])]
        );
        let me = &view.inputs[2].owners[0];
        assert!(me.local);
        assert_eq!(me.platform, Some(Platform::current()));
        assert_eq!(view.inputs[1].owners[0].platform, Some(Platform::Windows));
    }

    #[test]
    fn configured_code_wins_on_preferred_display() {
        let on = |current| {
            let view = display_view(
                &info(),
                None,
                current,
                &presets(),
                true,
                &machines(Some(0x1B)),
            );
            owned(&view)
                .into_iter()
                .map(|(l, _)| l.to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(on(Some(InputCode(0x0F))), ["HDMI1", "USB-C"]);
        assert_eq!(on(None), ["HDMI1", "USB-C"]);
    }

    #[test]
    fn no_local_owner_without_code() {
        let view = display_view(&info(), None, None, &presets(), false, &machines(None));
        assert_eq!(owned(&view), [("HDMI1", vec!["win"])]);
    }

    #[test]
    fn machines_sharing_a_code_share_a_button() {
        let shared = [
            (InputCode(0x0F), peer("mbp", Platform::Macos)),
            (InputCode(0x0F), peer("win", Platform::Windows)),
        ];
        let buttons = input_buttons(&presets(), None, &shared);
        let names: Vec<&str> = buttons[0].owners.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(names, ["mbp", "win"]);
        assert_eq!(buttons.len(), 3);
    }

    #[test]
    fn appends_unlisted_machine_code_once() {
        let machines = [
            (InputCode(0x12), peer("win", Platform::Windows)),
            (InputCode(0x12), peer("linux", Platform::Linux)),
        ];
        let buttons = input_buttons(&presets(), Some(InputCode(0x12)), &machines);
        assert_eq!(buttons.len(), 4);
        let last = buttons.last().unwrap();
        assert_eq!((last.label.as_str(), last.active), ("0x12", true));
        assert_eq!(last.owners.len(), 2);
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
            &Machines::default(),
        );
        assert_eq!(view.subtitle, "外接显示器 · DP · 2560×1440 · 60Hz");
        assert_eq!(view.name, "HP D27k");
        assert!(view.screen.as_ref().unwrap().hidpi);
        assert_eq!(view.current_label.as_deref(), Some("DP"));
    }

    #[test]
    fn subtitle_omits_missing_parts() {
        let none = Machines::default();
        let view = display_view(&info(), None, None, &presets(), false, &none);
        assert_eq!(view.subtitle, "外接显示器");
        let no_refresh = ScreenMode {
            refresh_hz: None,
            pixel_width: 2560,
            pixel_height: 1440,
            ..mode()
        };
        let view = display_view(&info(), Some(no_refresh), None, &presets(), false, &none);
        assert_eq!(view.subtitle, "外接显示器 · 2560×1440");
        assert!(!view.screen.unwrap().hidpi);
    }

    #[test]
    fn windows_mode_shows_physical_resolution_with_scale() {
        let none = Machines::default();
        let windows = ScreenMode {
            width: 3840,
            height: 2160,
            pixel_width: 3840,
            pixel_height: 2160,
            refresh_hz: Some(144.0),
            scale_percent: Some(175),
        };
        let view = display_view(&info(), Some(windows), None, &presets(), false, &none);
        assert_eq!(view.subtitle, "外接显示器 · 3840×2160 · 144Hz");
        let screen = view.screen.unwrap();
        assert!(!screen.hidpi);
        assert_eq!(screen.scale_percent, Some(175));
    }
}
