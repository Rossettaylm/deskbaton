use baton_core::ids::DeviceId;
use baton_core::{Error, Result};

use super::InputShareBackend;

/// 自研的输入捕获与注入（可参考 Lan Mouse 的各平台后端）。
pub struct NativeBackend;

impl InputShareBackend for NativeBackend {
    fn focus(&self, _target: Option<&DeviceId>) -> Result<()> {
        Err(Error::Unsupported("自研软件键鼠后端"))
    }
}
