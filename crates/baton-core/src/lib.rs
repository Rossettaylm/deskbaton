//! DeskBaton 核心层。
//!
//! 只包含与平台无关的逻辑：领域类型、协议消息、配置、端口接口（trait）和交棒协调器。
//! 各平台能力（DDC、电源、键鼠路由、网络）由其他 crate 实现 [`ports`] 中的 trait 后注入。

pub mod autostart;
pub mod channel;
pub mod config;
pub mod coordinator;
pub mod error;
pub mod event;
pub mod files;
pub mod ids;
pub mod learn;
pub mod outbox;
pub mod participant;
pub mod ports;
pub mod protocol;
pub mod state;
pub mod state_file;
pub mod wake;

pub use error::{Error, Result};
