//! 唤醒睡眠中的对端。每种方式实现 [`baton_core::ports::Waker`]。

mod hid_dongle;
mod prompt;
mod wol;

pub use hid_dongle::HidDongleWaker;
pub use prompt::PromptWaker;
pub use wol::{WOL_PORT, WolWaker, destinations, magic_packet};
