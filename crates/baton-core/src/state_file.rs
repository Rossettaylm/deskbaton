//! 数据目录下的 `state.toml`：程序自己维护的本机状态，用户不需要编辑。

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ids::InputCode;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalState {
    /// 自动学到的本机输入源编码。
    #[serde(default)]
    pub input_code: Option<InputCode>,
    /// 面板保存过的本机名称；`None` 表示未保存，空字符串表示用主机名。
    #[serde(default)]
    pub name: Option<String>,
    /// 面板保存过的交棒热键。
    #[serde(default)]
    pub hotkey: Option<String>,
    /// 是否弹出系统通知。
    #[serde(default = "default_notify")]
    pub notify: bool,
    /// 是否同步系统剪贴板。
    #[serde(default = "default_clipboard_sync")]
    pub clipboard_sync: bool,
    /// 兼容旧 `state.toml`；面板不再使用。
    #[serde(default = "default_clipboard_tab")]
    pub clipboard_tab: bool,
    /// 每个对端发件箱最多几条；启动时读取，面板不提供旋钮。
    #[serde(default = "default_clipboard_outbox_limit")]
    pub clipboard_outbox_limit: u32,
    /// 面板已经成功种过一次登录项；之后不再擅自打开开机自启。
    #[serde(default)]
    pub autostart_seeded: bool,
    /// 来自已配对设备的文件直接接收；关闭时请求挂起，等用户处理。
    #[serde(default = "default_file_auto_accept")]
    pub file_auto_accept: bool,
    /// 接收位置；空表示系统「下载」文件夹。
    #[serde(default)]
    pub file_receive_dir: Option<String>,
    /// 正被面板软件调暗的显示器；异常退出后下次启动据此复原。
    #[serde(default)]
    pub dimmed_displays: Vec<String>,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            input_code: None,
            name: None,
            hotkey: None,
            notify: default_notify(),
            clipboard_sync: default_clipboard_sync(),
            clipboard_tab: default_clipboard_tab(),
            clipboard_outbox_limit: default_clipboard_outbox_limit(),
            autostart_seeded: false,
            file_auto_accept: default_file_auto_accept(),
            file_receive_dir: None,
            dimmed_displays: Vec::new(),
        }
    }
}

fn default_notify() -> bool {
    true
}

fn default_file_auto_accept() -> bool {
    true
}

fn default_clipboard_sync() -> bool {
    true
}

fn default_clipboard_tab() -> bool {
    true
}

fn default_clipboard_outbox_limit() -> u32 {
    10
}

/// 合法范围 1–50，其它值当 10。
pub fn clamp_outbox_limit(n: u32) -> u32 {
    if (1..=50).contains(&n) { n } else { 10 }
}

impl LocalState {
    /// `state.toml` > 配置 > 主机名；空字符串表示用户清空了名称，只用主机名。
    pub fn name<'a>(&'a self, config: Option<&'a str>, hostname: &'a str) -> &'a str {
        match self.name.as_deref() {
            Some("") => hostname,
            Some(name) => name,
            None => config.unwrap_or(hostname),
        }
    }

    /// `state.toml` > 配置（配置侧已带默认热键）。
    pub fn hotkey<'a>(&'a self, config: &'a str) -> &'a str {
        self.hotkey.as_deref().unwrap_or(config)
    }

    /// 加载、修改、写回，避免只保存部分字段把其余偏好冲掉。
    pub fn update(path: &Path, f: impl FnOnce(&mut Self)) -> Result<Self> {
        let mut state = Self::load(path)?;
        f(&mut state);
        state.save(path)?;
        Ok(state)
    }

    /// 取出并清空上次留下的软件调光记录；没有记录时不写文件。
    pub fn take_dimmed(path: &Path) -> Result<Vec<String>> {
        if Self::load(path)?.dimmed_displays.is_empty() {
            return Ok(Vec::new());
        }
        let mut taken = Vec::new();
        Self::update(path, |s| taken = std::mem::take(&mut s.dimmed_displays))?;
        Ok(taken)
    }

    /// 文件不存在时为空状态。
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let mut state: Self = toml::from_str(&text)
                    .map_err(|e| Error::Config(format!("{} 有误：{e}", path.display())))?;
                state.clipboard_outbox_limit = clamp_outbox_limit(state.clipboard_outbox_limit);
                Ok(state)
            }
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(io_error(path, e)),
        }
    }

    /// 先写临时文件再重命名，崩溃时不会留下写了一半的文件。
    pub fn save(&self, path: &Path) -> Result<()> {
        let text =
            toml::to_string(self).map_err(|e| Error::Config(format!("无法序列化本机状态：{e}")))?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
        }
        let tmp = path.with_extension("toml.tmp");
        let mut file = fs::File::create(&tmp).map_err(|e| io_error(&tmp, e))?;
        file.write_all(text.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|e| io_error(&tmp, e))?;
        fs::rename(&tmp, path).map_err(|e| io_error(path, e))
    }
}

fn io_error(path: &Path, e: std::io::Error) -> Error {
    Error::Config(format!("读写 {} 失败：{e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_and_tolerates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        assert_eq!(LocalState::load(&path).unwrap(), LocalState::default());
        let state = LocalState {
            input_code: Some(InputCode(0x10)),
            ..LocalState::default()
        };
        state.save(&path).unwrap();
        assert_eq!(LocalState::load(&path).unwrap(), state);
        LocalState::default().save(&path).unwrap();
        assert_eq!(LocalState::load(&path).unwrap().input_code, None);
    }

    #[test]
    fn rejects_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "input_code = 16\nextra = 1\n").unwrap();
        assert!(LocalState::load(&path).is_err());
    }

    #[test]
    fn prefs_default_when_file_omits_them() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "input_code = 16\n").unwrap();
        let state = LocalState::load(&path).unwrap();
        assert_eq!(state.input_code, Some(InputCode(16)));
        assert_eq!(state.name, None);
        assert_eq!(state.hotkey, None);
        assert!(state.notify);
        assert!(state.clipboard_sync);
        assert!(state.clipboard_tab);
        assert_eq!(state.clipboard_outbox_limit, 10);
        assert!(!state.autostart_seeded);
        assert!(state.file_auto_accept);
        assert_eq!(state.file_receive_dir, None);
        assert!(state.dimmed_displays.is_empty());
    }

    #[test]
    fn dimmed_displays_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        LocalState::update(&path, |s| {
            s.dimmed_displays = vec!["A".into(), "B#2".into()]
        })
        .unwrap();
        assert_eq!(
            LocalState::load(&path).unwrap().dimmed_displays,
            ["A", "B#2"]
        );
        assert_eq!(LocalState::take_dimmed(&path).unwrap(), ["A", "B#2"]);
        assert!(LocalState::load(&path).unwrap().dimmed_displays.is_empty());
        assert!(LocalState::take_dimmed(&path).unwrap().is_empty());
    }

    #[test]
    fn take_dimmed_without_record_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        assert!(LocalState::take_dimmed(&path).unwrap().is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn clamp_outbox_limit_keeps_valid_and_defaults_invalid() {
        assert_eq!(clamp_outbox_limit(1), 1);
        assert_eq!(clamp_outbox_limit(10), 10);
        assert_eq!(clamp_outbox_limit(50), 50);
        assert_eq!(clamp_outbox_limit(0), 10);
        assert_eq!(clamp_outbox_limit(51), 10);
    }

    #[test]
    fn load_clamps_illegal_outbox_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        fs::write(&path, "clipboard_outbox_limit = 99\n").unwrap();
        assert_eq!(LocalState::load(&path).unwrap().clipboard_outbox_limit, 10);
    }

    #[test]
    fn name_prefers_state_then_config_then_hostname() {
        let hostname = "mini";
        let with_state = LocalState {
            name: Some("panel".into()),
            ..LocalState::default()
        };
        assert_eq!(with_state.name(Some("config"), hostname), "panel");

        let empty = LocalState {
            name: Some(String::new()),
            ..LocalState::default()
        };
        assert_eq!(empty.name(Some("config"), hostname), hostname);

        let unset = LocalState::default();
        assert_eq!(unset.name(Some("config"), hostname), "config");
        assert_eq!(unset.name(None, hostname), hostname);
    }

    #[test]
    fn hotkey_prefers_state_then_config() {
        let from_state = LocalState {
            hotkey: Some("Ctrl+Alt+B".into()),
            ..LocalState::default()
        };
        assert_eq!(from_state.hotkey("Ctrl+Alt+Space"), "Ctrl+Alt+B");
        assert_eq!(
            LocalState::default().hotkey("Ctrl+Alt+Space"),
            "Ctrl+Alt+Space"
        );
    }

    #[test]
    fn update_keeps_other_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");
        LocalState {
            input_code: Some(InputCode(0x10)),
            name: Some("macbook".into()),
            hotkey: Some("Ctrl+Alt+B".into()),
            notify: false,
            clipboard_sync: false,
            clipboard_tab: false,
            clipboard_outbox_limit: 3,
            autostart_seeded: true,
            file_auto_accept: false,
            file_receive_dir: Some("/tmp/inbox".into()),
            dimmed_displays: vec!["A".into()],
        }
        .save(&path)
        .unwrap();

        LocalState::update(&path, |state| state.input_code = None).unwrap();

        let loaded = LocalState::load(&path).unwrap();
        assert_eq!(loaded.input_code, None);
        assert_eq!(loaded.name.as_deref(), Some("macbook"));
        assert_eq!(loaded.hotkey.as_deref(), Some("Ctrl+Alt+B"));
        assert!(!loaded.notify);
        assert!(!loaded.clipboard_sync);
        assert!(!loaded.clipboard_tab);
        assert_eq!(loaded.clipboard_outbox_limit, 3);
        assert!(loaded.autostart_seeded);
        assert!(!loaded.file_auto_accept);
        assert_eq!(loaded.file_receive_dir.as_deref(), Some("/tmp/inbox"));
        assert_eq!(loaded.dimmed_displays, ["A"]);
    }
}
