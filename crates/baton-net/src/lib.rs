//! 局域网互联：mDNS 发现、配对、TLS 传输、长度前缀 JSON 分帧。

pub mod discovery;
pub mod frame;
pub mod pairing;

mod link;

pub use link::TcpPeerLink;
