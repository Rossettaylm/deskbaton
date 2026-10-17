use std::net::IpAddr;

use crate::channel::ChannelId;
use crate::ids::{DeviceId, HandoffId, InputCode, MacAddr, Platform};
use crate::participant::Payload;
use crate::protocol::Message;

/// 对端在握手时声明的信息，或设备档案里记下的信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub device_id: DeviceId,
    pub platform: Option<Platform>,
    pub input_code: Option<InputCode>,
    pub wol_mac: Option<MacAddr>,
    /// 对端最后已知的 IP，用于单播魔术包。
    pub ip: Option<IpAddr>,
}

/// 驱动协调器的全部输入：热键、链路事件、对端消息。
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Hotkey,
    /// 用户在面板上点了某台电脑：与热键走同一流程，目标由用户指定。
    HandoffTo(DeviceId),
    /// 本机输入源编码学到了，或被重新识别清空。
    LocalInputCode(Option<InputCode>),
    /// 握手完成。
    PeerOnline(PeerInfo),
    /// 在线对端的信息变了（如输入源编码）。`input_code` 按原值替换，其余字段为空时保留旧值。
    PeerUpdated(PeerInfo),
    /// 连续 3 次没收到心跳，或连接断开。
    PeerOffline(DeviceId),
    /// 用户取消了与该设备的配对。
    PeerRemoved(DeviceId),
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
