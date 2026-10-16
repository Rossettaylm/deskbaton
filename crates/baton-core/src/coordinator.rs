use crate::config::{Config, DisplayMode};
use crate::ports::{DisplayControl, DisplayInfo, InputRouter, PeerLink, PowerControl};
use crate::state::AgentState;
use crate::wake::WakeChain;
use crate::{Error, Result};

/// 注入给协调器的全部外部能力。
pub struct Ports {
    pub display: Box<dyn DisplayControl>,
    pub power: Box<dyn PowerControl>,
    pub input: Box<dyn InputRouter>,
    pub wake: WakeChain,
    pub link: Box<dyn PeerLink>,
}

/// 交棒协调器：维护本机状态，驱动交棒流程。
pub struct Coordinator {
    config: Config,
    ports: Ports,
    state: AgentState,
}

impl Coordinator {
    pub fn new(config: Config, ports: Ports) -> Self {
        Self {
            config,
            ports,
            state: AgentState::Idle,
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    /// 通过读取显示器当前输入源判断本机是否持棒：读到本机的输入源编码即为持棒。
    pub fn refresh_state(&mut self) -> Result<AgentState> {
        let mine = match self.config.display.mode {
            DisplayMode::Ddc => self
                .config
                .display
                .input_code
                .ok_or_else(|| Error::Config("未配置 display.input_code".into()))?,
            DisplayMode::None => return Err(Error::Unsupported("纯软件模式的持棒判定")),
        };
        let display =
            DisplayInfo::select(self.ports.display.list()?, &self.config.display.matcher)?;
        self.state = match self.ports.display.get_input(&display.id) {
            Ok(current) if current == mine => AgentState::Holding,
            _ => AgentState::Idle,
        };
        Ok(self.state)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::ids::{DeviceId, DisplayId, InputCode};
    use crate::ports::AwakeGuard;
    use crate::protocol::Message;

    struct FakeDisplay {
        displays: Vec<&'static str>,
        current: Result<InputCode>,
    }

    impl DisplayControl for FakeDisplay {
        fn list(&self) -> Result<Vec<DisplayInfo>> {
            Ok(self
                .displays
                .iter()
                .map(|model| DisplayInfo {
                    id: DisplayId((*model).into()),
                    vendor: "TA".into(),
                    model: (*model).into(),
                    serial: None,
                })
                .collect())
        }
        fn get_input(&self, _: &DisplayId) -> Result<InputCode> {
            match &self.current {
                Ok(code) => Ok(*code),
                Err(_) => Err(Error::Ddc("inactive input".into())),
            }
        }
        fn set_input(&self, _: &DisplayId, _: InputCode) -> Result<()> {
            Ok(())
        }
    }

    struct Noop;

    impl PowerControl for Noop {
        fn stay_awake(&self, _: Duration) -> Result<AwakeGuard> {
            Ok(AwakeGuard::noop())
        }
        fn wake_display(&self) -> Result<()> {
            Ok(())
        }
    }

    impl InputRouter for Noop {
        fn acquire(&self) -> Result<()> {
            Ok(())
        }
        fn release(&self, _: &DeviceId) -> Result<()> {
            Ok(())
        }
    }

    impl PeerLink for Noop {
        fn is_online(&self, _: &DeviceId) -> bool {
            false
        }
        fn send(&self, _: &DeviceId, _: &Message) -> Result<()> {
            Ok(())
        }
    }

    fn coordinator(display: FakeDisplay, input_code: Option<u16>) -> Coordinator {
        let mut config = Config::default();
        config.display.input_code = input_code.map(InputCode);
        Coordinator::new(
            config,
            Ports {
                display: Box::new(display),
                power: Box::new(Noop),
                input: Box::new(Noop),
                wake: WakeChain::from_config(&[], Vec::new()),
                link: Box::new(Noop),
            },
        )
    }

    #[test]
    fn holding_when_display_shows_our_input() {
        let mut c = coordinator(
            FakeDisplay {
                displays: vec!["P275MV"],
                current: Ok(InputCode(0x1B)),
            },
            Some(0x1B),
        );
        assert_eq!(c.refresh_state().unwrap(), AgentState::Holding);
    }

    #[test]
    fn idle_when_display_shows_other_input_or_ignores_us() {
        let mut c = coordinator(
            FakeDisplay {
                displays: vec!["P275MV"],
                current: Ok(InputCode(0x0F)),
            },
            Some(0x1B),
        );
        assert_eq!(c.refresh_state().unwrap(), AgentState::Idle);

        let mut c = coordinator(
            FakeDisplay {
                displays: vec!["P275MV"],
                current: Err(Error::Timeout),
            },
            Some(0x1B),
        );
        assert_eq!(c.refresh_state().unwrap(), AgentState::Idle);
    }

    #[test]
    fn auto_match_requires_exactly_one_display() {
        let mut c = coordinator(
            FakeDisplay {
                displays: vec!["A", "B"],
                current: Ok(InputCode(0x1B)),
            },
            Some(0x1B),
        );
        assert!(matches!(c.refresh_state(), Err(Error::Config(_))));
    }

    #[test]
    fn missing_input_code_is_a_config_error() {
        let mut c = coordinator(
            FakeDisplay {
                displays: vec!["P275MV"],
                current: Ok(InputCode(0x1B)),
            },
            None,
        );
        assert!(matches!(c.refresh_state(), Err(Error::Config(_))));
    }
}
