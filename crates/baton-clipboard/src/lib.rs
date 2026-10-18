//! 系统剪贴板访问与实时同步。

mod access;
mod bridge;
mod echo;
mod sync;

pub use access::SystemClipboard;
pub use bridge::ClipboardBridge;
pub use echo::EchoSet;
pub use sync::{ClipboardSync, MAX_CLIPBOARD_BYTES};
