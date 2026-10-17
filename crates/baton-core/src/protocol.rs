use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, HandoffId, InputCode, MacAddr};
use crate::state::AgentState;

pub const PROTOCOL_VERSION: u32 = 3;

/// agent 之间的消息。交棒流程见 `docs/superpowers/specs/2026-10-16-deskbaton-design.md`，
/// 握手与配对见 `docs/superpowers/specs/2026-10-17-pairing-design.md`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        device_id: DeviceId,
        protocol_version: u32,
        input_code: Option<InputCode>,
        /// 承载本连接的网卡当前的 MAC。
        wol_mac: Option<MacAddr>,
        /// 本机监听端口，对端据此记下可回拨的地址。
        listen_port: u16,
        /// 预共享令牌对本次 TLS 会话双方证书指纹的 HMAC，十六进制；未配置令牌时省略。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth: Option<String>,
    },
    /// 拨号方对自己随机数的承诺：`SHA256(nonce)`，十六进制。
    PairCommit {
        commit: String,
    },
    /// 接受方在收到承诺后给出的随机数，十六进制。
    PairNonce {
        nonce: String,
    },
    /// 拨号方公开承诺过的随机数，十六进制。
    PairReveal {
        nonce: String,
    },
    /// 需要本机用户确认配对；对方也应显示配对码。
    PairPending,
    /// 已信任对方。双方都发出并收到后连接才建立。
    PairAccept,
    Heartbeat {
        state: AgentState,
    },
    Prepare {
        handoff_id: HandoffId,
    },
    Ready {
        handoff_id: HandoffId,
    },
    Taken {
        handoff_id: HandoffId,
    },
    Abort {
        handoff_id: HandoffId,
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_roundtrip_through_json() {
        let messages = [
            Message::Hello {
                device_id: DeviceId("mac".into()),
                protocol_version: PROTOCOL_VERSION,
                input_code: Some(InputCode(0x1B)),
                wol_mac: None,
                listen_port: 47800,
                auth: Some("00ff".into()),
            },
            Message::PairCommit {
                commit: "ab".into(),
            },
            Message::PairPending,
            Message::PairAccept,
            Message::Heartbeat {
                state: AgentState::HandingOff(HandoffId(7)),
            },
            Message::Abort {
                handoff_id: HandoffId(7),
                reason: "timeout".into(),
            },
        ];
        for msg in messages {
            let json = serde_json::to_string(&msg).unwrap();
            assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), msg);
        }
    }

    #[test]
    fn wire_format_is_stable() {
        let json = serde_json::to_string(&Message::Prepare {
            handoff_id: HandoffId(1),
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"prepare","handoff_id":1}"#);

        let hello = Message::Hello {
            device_id: DeviceId("mac".into()),
            protocol_version: 3,
            input_code: Some(InputCode(0x1B)),
            wol_mac: Some("aa:bb:cc:dd:ee:ff".parse().unwrap()),
            listen_port: 47800,
            auth: None,
        };
        assert_eq!(
            serde_json::to_string(&hello).unwrap(),
            r#"{"type":"hello","device_id":"mac","protocol_version":3,"input_code":27,"wol_mac":"aa:bb:cc:dd:ee:ff","listen_port":47800}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::PairAccept).unwrap(),
            r#"{"type":"pair_accept"}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::PairNonce { nonce: "01".into() }).unwrap(),
            r#"{"type":"pair_nonce","nonce":"01"}"#
        );
    }
}
