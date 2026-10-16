use baton_core::ids::DeviceId;
use baton_core::{Error, Result};

use super::InputShareBackend;

/// 以独立进程驱动 `deskflow-core`，保持与 GPL-2.0 的 Deskflow 进程隔离。
pub struct DeskflowBackend;

impl InputShareBackend for DeskflowBackend {
    fn focus(&self, _target: Option<&DeviceId>) -> Result<()> {
        Err(Error::Unsupported("Deskflow 软件键鼠后端"))
    }
}
