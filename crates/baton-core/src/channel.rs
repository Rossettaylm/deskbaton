use serde::{Deserialize, Serialize};

/// 逻辑通道号，对应帧头的 `channel` 字节。一对 agent 之间只有一条连接，各通道复用它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ChannelId(pub u8);

impl ChannelId {
    /// 交棒、心跳、状态，负载为 JSON [`crate::protocol::Message`]。
    pub const CONTROL: Self = Self(0);
    /// 剪贴板随身数据。
    pub const CLIPBOARD: Self = Self(1);
    /// 文件快传，只出现在文件连接上。
    pub const FILE: Self = Self(2);
}
