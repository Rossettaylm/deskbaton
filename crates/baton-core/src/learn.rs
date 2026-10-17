//! 本机输入源编码的自动学习，见 `docs/superpowers/specs/2026-10-17-zero-config-design.md` 第 4 节。

use std::time::Duration;

use crate::Result;
use crate::ids::InputCode;
use crate::ports::InputActivity;

/// 键鼠输入距今不超过这么久，才认为画面在本机。
pub const ACTIVE_WITHIN: Duration = Duration::from_secs(2);

/// 本机编码的来源：配置优先，其次学到的值。
#[derive(Debug, Clone, Default)]
pub struct InputCodeLearner {
    configured: Option<InputCode>,
    learned: Option<InputCode>,
}

impl InputCodeLearner {
    pub fn new(configured: Option<InputCode>, learned: Option<InputCode>) -> Self {
        Self {
            configured,
            learned,
        }
    }

    pub fn current(&self) -> Option<InputCode> {
        self.configured.or(self.learned)
    }

    pub fn is_configured(&self) -> bool {
        self.configured.is_some()
    }

    /// 尚未知道本机编码且本机刚有键鼠输入时读一次显示器；返回新学到的编码。
    /// 已知时不读，避免持续占用 DDC。
    pub fn poll(
        &mut self,
        activity: &dyn InputActivity,
        read_input: impl FnOnce() -> Result<InputCode>,
    ) -> Option<InputCode> {
        if self.current().is_some() {
            return None;
        }
        match activity.idle() {
            Ok(idle) if idle <= ACTIVE_WITHIN => {}
            Ok(_) => return None,
            Err(e) => {
                log::debug!("读取键鼠活动失败：{e}");
                return None;
            }
        }
        match read_input() {
            Ok(code) => {
                self.learned = Some(code);
                Some(code)
            }
            Err(e) => {
                log::debug!("本机有键鼠输入，但读不到显示器输入源：{e}");
                None
            }
        }
    }

    /// 忘掉学到的编码，等下次键鼠输入时重新学习。配置了编码时不起作用，返回 false。
    pub fn reset(&mut self) -> bool {
        if self.is_configured() {
            return false;
        }
        self.learned = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::Error;

    struct Idle(Duration);

    impl InputActivity for Idle {
        fn idle(&self) -> Result<Duration> {
            Ok(self.0)
        }
    }

    struct Broken;

    impl InputActivity for Broken {
        fn idle(&self) -> Result<Duration> {
            Err(Error::Unsupported("键鼠活动"))
        }
    }

    const ACTIVE: Idle = Idle(Duration::from_millis(500));
    const AWAY: Idle = Idle(Duration::from_secs(60));

    #[test]
    fn learns_when_recently_active_and_display_answers() {
        let mut learner = InputCodeLearner::default();
        assert_eq!(
            learner.poll(&ACTIVE, || Ok(InputCode(0x10))),
            Some(InputCode(0x10))
        );
        assert_eq!(learner.current(), Some(InputCode(0x10)));
    }

    #[test]
    fn configured_code_wins_and_is_never_read() {
        let mut learner = InputCodeLearner::new(Some(InputCode(0x0F)), Some(InputCode(0x10)));
        let read = Cell::new(false);
        let learned = learner.poll(&ACTIVE, || {
            read.set(true);
            Ok(InputCode(0x11))
        });
        assert_eq!(learned, None);
        assert!(!read.get());
        assert_eq!(learner.current(), Some(InputCode(0x0F)));
        assert!(!learner.reset());
        assert_eq!(learner.current(), Some(InputCode(0x0F)));
    }

    #[test]
    fn known_code_is_not_read_again() {
        let mut learner = InputCodeLearner::new(None, Some(InputCode(0x10)));
        let read = Cell::new(false);
        assert_eq!(
            learner.poll(&ACTIVE, || {
                read.set(true);
                Ok(InputCode(0x0F))
            }),
            None
        );
        assert!(!read.get());
    }

    #[test]
    fn idle_or_unreadable_does_not_learn() {
        let mut learner = InputCodeLearner::default();
        assert_eq!(learner.poll(&AWAY, || Ok(InputCode(0x10))), None);
        assert_eq!(learner.poll(&Broken, || Ok(InputCode(0x10))), None);
        assert_eq!(
            learner.poll(&ACTIVE, || Err(Error::Ddc("Null Message".into()))),
            None
        );
        assert_eq!(learner.current(), None);
    }

    #[test]
    fn reset_forgets_and_relearns() {
        let mut learner = InputCodeLearner::new(None, Some(InputCode(0x10)));
        assert!(learner.reset());
        assert_eq!(learner.current(), None);
        assert_eq!(
            learner.poll(&ACTIVE, || Ok(InputCode(0x0F))),
            Some(InputCode(0x0F))
        );
    }
}
