use serde::{Deserialize, Serialize};

use crate::ids::{InputCode, MacAddr};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub input: InputRouting,
    #[serde(default)]
    pub power: PowerConfig,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
    #[serde(default = "default_handoff_timeout_ms")]
    pub handoff_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey(),
            display: DisplayConfig::default(),
            input: InputRouting::default(),
            power: PowerConfig::default(),
            peers: Vec::new(),
            handoff_timeout_ms: default_handoff_timeout_ms(),
        }
    }
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| Error::Config(e.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayConfig {
    #[serde(default)]
    pub mode: DisplayMode,
    /// `"auto"` 表示唯一一台外接显示器，否则按 EDID 型号匹配。
    #[serde(default = "default_display_match", rename = "match")]
    pub matcher: String,
    /// 本机在显示器上对应的输入源编码。
    pub input_code: Option<InputCode>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::default(),
            matcher: default_display_match(),
            input_code: None,
        }
    }
}

/// 交棒时是否切换显示器输入源。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// 通过 DDC/CI 切换输入源（一台显示器多台电脑）。
    #[default]
    Ddc,
    /// 不切屏（每台电脑各有显示器，只移动键鼠）。
    None,
}

/// 键鼠如何跟随控制权移动。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputRouting {
    /// 键鼠插在显示器 USB 口上，由显示器内置 KVM 随输入源切换，软件不介入。
    #[default]
    MonitorKvm,
    /// 键鼠插在某台电脑上，由软件把输入转发给持棒方。
    Software { backend: SoftwareBackend },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoftwareBackend {
    /// 以独立进程驱动 Deskflow。
    Deskflow,
    /// 自研的捕获与注入实现。
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerConfig {
    /// 交出控制权后保持唤醒到什么时候。
    #[serde(default)]
    pub stay_awake_until: StayAwakeUntil,
    /// `duration` 模式下交棒后本机保持唤醒的秒数；0 表示立即允许系统睡眠。
    #[serde(default = "default_stay_awake_secs")]
    pub stay_awake_secs: u64,
    /// 接棒方离线时按顺序尝试的唤醒方式。
    #[serde(default = "default_wake_chain")]
    pub wake_chain: Vec<WakeMethod>,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            stay_awake_until: StayAwakeUntil::default(),
            stay_awake_secs: default_stay_awake_secs(),
            wake_chain: default_wake_chain(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StayAwakeUntil {
    /// 一直保持到接棒方离线（关机、睡眠）。
    #[default]
    PeerOffline,
    /// 保持 `stay_awake_secs` 秒。
    Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeMethod {
    /// Wake-on-LAN 魔术包，适合有线网络的电脑。
    Wol,
    /// 常插在目标电脑上的 USB / BLE HID 配件，模拟一次按键。
    HidDongle,
    /// 提示用户手动唤醒（例如开盖）。
    Prompt,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeerConfig {
    pub name: String,
    /// mDNS 发现失败时使用的兜底地址，如 `192.168.1.20:47800`。
    pub address: Option<String>,
    pub wol_mac: Option<MacAddr>,
}

fn default_hotkey() -> String {
    "Ctrl+Alt+Space".to_owned()
}

fn default_display_match() -> String {
    "auto".to_owned()
}

fn default_handoff_timeout_ms() -> u64 {
    8_000
}

fn default_stay_awake_secs() -> u64 {
    300
}

fn default_wake_chain() -> Vec<WakeMethod> {
    vec![WakeMethod::Wol, WakeMethod::HidDongle, WakeMethod::Prompt]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        assert_eq!(Config::from_toml_str("").unwrap(), Config::default());
    }

    #[test]
    fn parses_monitor_kvm_setup() {
        let cfg = Config::from_toml_str(
            r#"
            hotkey = "Ctrl+Alt+B"

            [display]
            input_code = 0x1B

            [power]
            stay_awake_until = "duration"
            stay_awake_secs = 0
            wake_chain = ["hid_dongle", "prompt"]

            [[peers]]
            name = "windows-desktop"
            address = "192.168.1.20:47800"
            wol_mac = "aa:bb:cc:dd:ee:ff"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.display.mode, DisplayMode::Ddc);
        assert_eq!(cfg.display.input_code, Some(InputCode(0x1B)));
        assert_eq!(cfg.input, InputRouting::MonitorKvm);
        assert_eq!(cfg.power.stay_awake_until, StayAwakeUntil::Duration);
        assert_eq!(cfg.power.stay_awake_secs, 0);
        assert_eq!(
            cfg.peers[0].wol_mac,
            Some("aa:bb:cc:dd:ee:ff".parse().unwrap())
        );
    }

    #[test]
    fn parses_software_routing() {
        let cfg = Config::from_toml_str(
            r#"
            [display]
            mode = "none"

            [input]
            mode = "software"
            backend = "deskflow"
            "#,
        )
        .unwrap();

        assert_eq!(cfg.display.mode, DisplayMode::None);
        assert_eq!(
            cfg.input,
            InputRouting::Software {
                backend: SoftwareBackend::Deskflow
            }
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Config::from_toml_str("hotkye = \"x\"").is_err());
    }
}
