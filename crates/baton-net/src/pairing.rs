//! 配对：首次连接时双方显示同一个由证书指纹派生的 6 位数字，用户确认后固定对方指纹。

use baton_core::ids::DeviceId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairedPeer {
    pub device_id: DeviceId,
    /// 对端 TLS 证书的 SHA-256 指纹（十六进制）。
    pub cert_fingerprint: String,
}
