use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ids::{InputCode, MacAddr};
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 本机名称，即对端配置里 `[[peers]].name`；不填时取主机名。
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_hotkey")]
    pub hotkey: String,
    #[serde(default)]
    pub display: DisplayConfig,
    #[serde(default)]
    pub input: InputRouting,
    #[serde(default)]
    pub power: PowerConfig,
    #[serde(default)]
    pub network: NetworkConfig,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
    #[serde(default = "default_handoff_timeout_ms")]
    pub handoff_timeout_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            name: None,
            hotkey: default_hotkey(),
            display: DisplayConfig::default(),
            input: InputRouting::default(),
            power: PowerConfig::default(),
            network: NetworkConfig::default(),
            peers: Vec::new(),
            handoff_timeout_ms: default_handoff_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkConfig {
    /// 本机监听端口。
    #[serde(default = "default_port")]
    pub port: u16,
    /// 可选的预共享令牌：两端相同时首次连接自动配对，不需要在面板上确认。
    #[serde(default)]
    pub token: Option<String>,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            port: default_port(),
            token: None,
        }
    }
}

fn default_port() -> u16 {
    47800
}

impl Config {
    pub fn from_toml_str(s: &str) -> Result<Self> {
        toml::from_str(s).map_err(|e| Error::Config(e.to_string()))
    }
}

/// agent 与显示器面板共用的配置文件路径。
pub fn default_config_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("config.toml"))
}

/// 配置文件、本机身份、设备档案所在的目录。
pub fn data_dir() -> Result<PathBuf> {
    let env = |key: &str| {
        std::env::var_os(key).map(PathBuf::from).ok_or_else(|| {
            Error::Config(format!("环境变量 {key} 未设置，请用 --config 指定配置文件"))
        })
    };
    #[cfg(target_os = "macos")]
    return Ok(env("HOME")?.join("Library/Application Support/DeskBaton"));
    #[cfg(target_os = "windows")]
    return Ok(env("APPDATA")?.join("DeskBaton"));
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    return Ok(env("XDG_CONFIG_HOME")
        .or_else(|_| env("HOME").map(|home| home.join(".config")))?
        .join("deskbaton"));
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
    /// 显示器面板上的输入源按钮。编码因显示器而异，默认值只是 MCCS 常见值。
    #[serde(default = "default_display_inputs")]
    pub inputs: Vec<InputPreset>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            mode: DisplayMode::default(),
            matcher: default_display_match(),
            input_code: None,
            inputs: default_display_inputs(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputPreset {
    pub label: String,
    pub code: InputCode,
}

impl InputPreset {
    pub fn new(label: &str, code: u16) -> Self {
        Self {
            label: label.to_owned(),
            code: InputCode(code),
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
    /// 何时停止保持唤醒。
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
    /// 任一已配对对端在线就保持，自己持棒也不释放；最后一个对端离线才恢复睡眠。
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

fn default_display_inputs() -> Vec<InputPreset> {
    vec![
        InputPreset::new("DP", 0x0F),
        InputPreset::new("HDMI1", 0x11),
        InputPreset::new("HDMI2", 0x12),
        InputPreset::new("USB-C", 0x1B),
    ]
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
            name = "macbook"
            hotkey = "Ctrl+Alt+B"

            [network]
            token = "s3cret"

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

        assert_eq!(cfg.name.as_deref(), Some("macbook"));
        assert_eq!(cfg.network.port, 47800);
        assert_eq!(cfg.network.token.as_deref(), Some("s3cret"));
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
    fn display_inputs_default_to_common_mccs_codes() {
        let cfg = Config::from_toml_str("[display]\ninput_code = 0x1B").unwrap();
        let labels: Vec<&str> = cfg
            .display
            .inputs
            .iter()
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(labels, ["DP", "HDMI1", "HDMI2", "USB-C"]);
        assert_eq!(cfg.display.inputs[3].code, InputCode(0x1B));
    }

    #[test]
    fn display_inputs_can_be_customized() {
        let cfg = Config::from_toml_str(
            r#"
            [[display.inputs]]
            label = "Type-C"
            code = 0x19

            [[display.inputs]]
            label = "DP"
            code = 0x0F
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.display.inputs,
            [
                InputPreset::new("Type-C", 0x19),
                InputPreset::new("DP", 0x0F)
            ]
        );
        assert!(
            Config::from_toml_str("[[display.inputs]]\nlabel = \"DP\"\ncode = 15\nname = \"x\"")
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(Config::from_toml_str("hotkye = \"x\"").is_err());
    }
}
