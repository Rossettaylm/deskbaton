use crate::Result;
use crate::channel::ChannelId;
use crate::ids::DeviceId;

/// 交棒时随身带给接棒方的一份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Payload {
    /// 由参与者自行约定，如 `text/plain`、`image/png`。
    pub format: String,
    pub data: Vec<u8>,
}

/// 交棒参与者：交出控制权时打包数据，接棒方收到完整数据后解包。失败只记日志，不影响交棒。
pub trait HandoffParticipant: Send + Sync {
    fn channel(&self) -> ChannelId;
    /// 交出控制权前调用；`None` 表示本次不带。
    fn pack(&self, to: &DeviceId) -> Result<Option<Payload>>;
    /// 接棒方收到完整数据后调用，可能早于或晚于交棒完成。
    fn unpack(&self, from: &DeviceId, payload: Payload) -> Result<()>;
}
