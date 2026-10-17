//! 系统剪贴板访问与交棒随身数据。

mod access;
mod sync;

pub use access::SystemClipboard;
pub use sync::{ClipboardSync, MAX_CLIPBOARD_BYTES};
