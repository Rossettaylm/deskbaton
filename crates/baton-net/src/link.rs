use baton_core::channel::ChannelId;
use baton_core::ids::{DeviceId, HandoffId};
use baton_core::participant::Payload;
use baton_core::ports::PeerLink;
use baton_core::protocol::Message;
use baton_core::state::AgentState;
use baton_core::{Error, Result};

/// TCP + TLS 的对端链路。
pub struct TcpPeerLink;

impl PeerLink for TcpPeerLink {
    fn send(&self, _to: &DeviceId, _msg: &Message) -> Result<()> {
        Err(Error::Unsupported("网络链路"))
    }

    fn send_data(&self, _: &DeviceId, _: ChannelId, _: HandoffId, _: Payload) -> Result<()> {
        Err(Error::Unsupported("网络链路"))
    }

    fn set_local_state(&self, _state: AgentState) {}
}
