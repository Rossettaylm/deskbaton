//! 数据目录下的 `state.toml`：程序自己维护的本机状态，用户不需要编辑。

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::ids::InputCode;
use crate::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalState {
    /// 自动学到的本机输入源编码。
    #[serde(default)]
    pub input_code: Option<InputCode>,
}

impl LocalState {
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
}
