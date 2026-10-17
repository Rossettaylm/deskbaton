use crate::config::WakeMethod;
use crate::ports::{WakeOutcome, WakeTarget, Waker};
use crate::{Error, Result};

/// 按配置顺序依次尝试多种唤醒方式。
pub struct WakeChain {
    wakers: Vec<Box<dyn Waker>>,
}

impl WakeChain {
    /// 从 `available` 中按 `order` 挑选并排序；配置了但当前平台没有实现的方式会被跳过。
    pub fn from_config(order: &[WakeMethod], available: Vec<Box<dyn Waker>>) -> Self {
        let mut pool: Vec<Option<Box<dyn Waker>>> = available.into_iter().map(Some).collect();
        let wakers = order
            .iter()
            .filter_map(|method| {
                pool.iter_mut()
                    .find(|slot| slot.as_ref().is_some_and(|w| w.method() == *method))
                    .and_then(Option::take)
            })
            .collect();
        Self { wakers }
    }

    pub fn methods(&self) -> Vec<WakeMethod> {
        self.wakers.iter().map(|w| w.method()).collect()
    }

    /// 返回第一个成功的结果；`NeedsUser` 也视为终点，不再继续尝试后面的方式。
    pub fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome> {
        let mut errors = Vec::new();
        for waker in &self.wakers {
            match waker.wake(target) {
                Ok(outcome) => return Ok(outcome),
                Err(e) => errors.push(format!("{:?}: {e}", waker.method())),
            }
        }
        Err(Error::Wake(if errors.is_empty() {
            "没有可用的唤醒方式".to_owned()
        } else {
            errors.join("; ")
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::DeviceId;

    struct Fake(WakeMethod, fn() -> Result<WakeOutcome>);

    impl Waker for Fake {
        fn method(&self) -> WakeMethod {
            self.0
        }
        fn wake(&self, _: &WakeTarget) -> Result<WakeOutcome> {
            (self.1)()
        }
    }

    fn target() -> WakeTarget {
        WakeTarget {
            device_id: DeviceId("mac".into()),
            wol_mac: None,
            last_ip: None,
        }
    }

    #[test]
    fn orders_by_config_and_skips_missing() {
        let chain = WakeChain::from_config(
            &[WakeMethod::HidDongle, WakeMethod::Wol, WakeMethod::Prompt],
            vec![
                Box::new(Fake(WakeMethod::Prompt, || Ok(WakeOutcome::Sent))),
                Box::new(Fake(WakeMethod::Wol, || Ok(WakeOutcome::Sent))),
            ],
        );
        assert_eq!(chain.methods(), vec![WakeMethod::Wol, WakeMethod::Prompt]);
    }

    #[test]
    fn falls_through_failures_until_success() {
        let chain = WakeChain::from_config(
            &[WakeMethod::Wol, WakeMethod::Prompt],
            vec![
                Box::new(Fake(WakeMethod::Wol, || {
                    Err(Error::Wake("no route".into()))
                })),
                Box::new(Fake(WakeMethod::Prompt, || {
                    Ok(WakeOutcome::NeedsUser("请开盖".into()))
                })),
            ],
        );
        assert_eq!(
            chain.wake(&target()).unwrap(),
            WakeOutcome::NeedsUser("请开盖".into())
        );
    }

    #[test]
    fn empty_chain_is_an_error() {
        let chain = WakeChain::from_config(&[WakeMethod::Wol], Vec::new());
        assert!(chain.wake(&target()).is_err());
    }
}
