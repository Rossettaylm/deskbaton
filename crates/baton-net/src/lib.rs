//! 局域网互联：mDNS 发现、配对、TLS 传输、带通道号的分帧与发送调度。

pub mod discovery;
pub mod frame;
pub mod mux;
pub mod pairing;
pub mod transfer;

mod link;

pub use link::TcpPeerLink;
