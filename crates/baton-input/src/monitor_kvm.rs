use baton_core::Result;
use baton_core::ids::DeviceId;
use baton_core::ports::InputRouter;

pub struct MonitorKvmRouter;

impl InputRouter for MonitorKvmRouter {
    fn acquire(&self) -> Result<()> {
        Ok(())
    }

    fn release(&self, _to: &DeviceId) -> Result<()> {
        Ok(())
    }
}
