mod deskflow;
mod native;

use baton_core::Result;
use baton_core::config::SoftwareBackend;
use baton_core::ids::DeviceId;
use baton_core::ports::InputRouter;

/// 软件键鼠共享的具体实现。
pub trait InputShareBackend: Send + Sync {
    /// 把物理键鼠的输入导向 `target`；`None` 表示收回到本机。
    fn focus(&self, target: Option<&DeviceId>) -> Result<()>;
}

pub struct SoftwareRouter {
    backend: Box<dyn InputShareBackend>,
}

impl SoftwareRouter {
    pub fn new(kind: SoftwareBackend) -> Self {
        let backend: Box<dyn InputShareBackend> = match kind {
            SoftwareBackend::Deskflow => Box::new(deskflow::DeskflowBackend),
            SoftwareBackend::Native => Box::new(native::NativeBackend),
        };
        Self { backend }
    }
}

impl InputRouter for SoftwareRouter {
    fn acquire(&self) -> Result<()> {
        self.backend.focus(None)
    }

    fn release(&self, to: &DeviceId) -> Result<()> {
        self.backend.focus(Some(to))
    }
}
