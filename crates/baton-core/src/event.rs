use crate::channel::ChannelId;
use crate::ids::{DeviceId, HandoffId, InputCode, MacAddr};
use crate::participant::Payload;
use crate::protocol::Message;

/// 对端在握手时声明的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub device_id: DeviceId,
    pub input_code: Option<InputCode>,
    pub wol_mac: Option<MacAddr>,
}

/// 驱动协调器的全部输入：热键、链路事件、对端消息。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Hotkey,
    /// 握手完成。
    PeerOnline(PeerInfo),
    /// 连续 3 次没收到心跳，或连接断开。
    PeerOffline(DeviceId),
    Message {
        from: DeviceId,
        message: Message,
    },
    /// 数据通道上一次完整、校验通过的传输。
    Data {
        from: DeviceId,
        channel: ChannelId,
        handoff_id: HandoffId,
        payload: Payload,
    },
    Shutdown,
}
