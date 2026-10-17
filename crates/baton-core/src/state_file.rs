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
    /// 面板已经成功种过一次登录项；之后不再擅自打开开机自启。
    #[serde(default)]
    pub autostart_seeded: bool,
}

impl Default for LocalState {
    fn default() -> Self {
        Self {
            input_code: None,
            name: None,
            hotkey: None,
            notify: default_notify(),
            autostart_seeded: false,
        }
    }
}

fn default_notify() -> bool {
    true
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

    /// 文件不存在时为空状态。
    pub fn load(path: &Path) -> Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| Error::Config(format!("{} 有误：{e}", path.display()))),
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
        assert!(!state.autostart_seeded);
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
            autostart_seeded: true,
        }
        .save(&path)
        .unwrap();

        LocalState::update(&path, |state| state.input_code = None).unwrap();

        let loaded = LocalState::load(&path).unwrap();
        assert_eq!(loaded.input_code, None);
        assert_eq!(loaded.name.as_deref(), Some("macbook"));
        assert_eq!(loaded.hotkey.as_deref(), Some("Ctrl+Alt+B"));
        assert!(!loaded.notify);
        assert!(loaded.autostart_seeded);
    }
}
