use baton_core::ids::DeviceId;
use baton_core::ports::PeerLink;
use baton_core::protocol::Message;
use baton_core::{Error, Result};

/// TCP + TLS 的对端链路。
pub struct TcpPeerLink;

impl PeerLink for TcpPeerLink {
    fn is_online(&self, _peer: &DeviceId) -> bool {
        false
    }

    fn send(&self, _to: &DeviceId, _msg: &Message) -> Result<()> {
        Err(Error::Unsupported("网络链路"))
    }
}
