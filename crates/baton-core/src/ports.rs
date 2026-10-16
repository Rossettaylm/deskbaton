//! 端口接口：核心层依赖的外部能力。各平台 crate 实现这些 trait，由 agent 组装后注入 [`crate::coordinator::Coordinator`]。

use std::time::Duration;

use crate::config::WakeMethod;
use crate::ids::{DeviceId, DisplayId, InputCode, MacAddr};
use crate::protocol::Message;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub id: DisplayId,
    pub vendor: String,
    pub model: String,
    pub serial: Option<String>,
}

impl DisplayInfo {
    /// `matcher` 为 `"auto"` 时要求恰好一台显示器，否则按型号子串匹配。
    pub fn select(displays: Vec<DisplayInfo>, matcher: &str) -> Result<DisplayInfo> {
        if matcher == "auto" {
            return match <[DisplayInfo; 1]>::try_from(displays) {
                Ok([only]) => Ok(only),
                Err(displays) if displays.is_empty() => {
                    Err(Error::Ddc("未发现支持 DDC/CI 的外接显示器".into()))
                }
                Err(_) => Err(Error::Config(
                    "发现多台显示器，请指定型号（display.match）".into(),
                )),
            };
        }
        displays
            .into_iter()
            .find(|d| d.model.contains(matcher))
            .ok_or_else(|| Error::Config(format!("没有型号包含「{matcher}」的显示器")))
    }
}

/// 显示器输入源控制（DDC/CI）。
pub trait DisplayControl: Send + Sync {
    fn list(&self) -> Result<Vec<DisplayInfo>>;
    /// 多数显示器只在本机是当前活跃输入时才响应。
    fn get_input(&self, display: &DisplayId) -> Result<InputCode>;
    fn set_input(&self, display: &DisplayId, input: InputCode) -> Result<()>;
}

/// 本机电源控制。
pub trait PowerControl: Send + Sync {
    /// 在 `duration` 内阻止本机睡眠（包括合盖睡眠）。到期或 guard 被释放后恢复系统原有策略。
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard>;
    /// 点亮本机显示输出，接棒前调用。
    fn wake_display(&self) -> Result<()>;
}

/// 释放时恢复系统睡眠策略。
pub struct AwakeGuard {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl AwakeGuard {
    pub fn new(release: impl FnOnce() + Send + 'static) -> Self {
        Self {
            release: Some(Box::new(release)),
        }
    }

    pub fn noop() -> Self {
        Self { release: None }
    }
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeTarget {
    pub device_id: DeviceId,
    pub wol_mac: Option<MacAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WakeOutcome {
    /// 唤醒信号已发出，需等待对端心跳恢复来确认。
    Sent,
    /// 无法自动唤醒，需要用户操作（附提示文案）。
    NeedsUser(String),
}

/// 唤醒睡眠中的对端。
pub trait Waker: Send + Sync {
    fn method(&self) -> WakeMethod;
    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome>;
}

/// 键鼠路由：控制权移动时让键鼠跟随。
pub trait InputRouter: Send + Sync {
    /// 本机成为持棒方后调用。
    fn acquire(&self) -> Result<()>;
    /// 本机把控制权交给 `to` 之前调用。
    fn release(&self, to: &DeviceId) -> Result<()>;
}

/// 与其他 agent 的通信链路。
pub trait PeerLink: Send + Sync {
    fn is_online(&self, peer: &DeviceId) -> bool;
    fn send(&self, to: &DeviceId, msg: &Message) -> Result<()>;
}
