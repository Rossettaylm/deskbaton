use baton_core::Result;
use baton_core::config::WakeMethod;
use baton_core::ports::{WakeOutcome, WakeTarget, Waker};

/// 兜底：提示用户手动唤醒。
pub struct PromptWaker;

impl Waker for PromptWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::Prompt
    }

    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome> {
        Ok(WakeOutcome::NeedsUser(format!(
            "{} 处于睡眠且无法自动唤醒，请手动唤醒（合盖的 Mac 需要开盖）",
            target.device_id.0
        )))
    }
}
