use serde::{Deserialize, Serialize};

use crate::files::{OfferSummary, RejectReason, SessionId};
use crate::ids::{DeviceId, HandoffId, InputCode, MacAddr, Platform};
use crate::state::AgentState;

pub const PROTOCOL_VERSION: u32 = 6;

/// agent 之间的消息。交棒流程见 `docs/superpowers/specs/2026-10-16-deskbaton-design.md`，
/// 握手与配对见 `docs/superpowers/specs/2026-10-17-pairing-design.md`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        device_id: DeviceId,
        protocol_version: u32,
        platform: Platform,
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
    /// 本机输入源编码变了：学到新值，或被重新识别清空（`null`）。
    InputCode {
        input_code: Option<InputCode>,
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
    /// 发送方的请求摘要，见 `docs/superpowers/specs/2026-10-19-file-transfer-design.md`。
    FileOffer {
        session_id: SessionId,
        #[serde(flatten)]
        summary: OfferSummary,
    },
    /// 接收方同意；`token` 为文件连接要出示的一次性令牌，十六进制。
    FileAccept {
        session_id: SessionId,
        token: String,
    },
    FileReject {
        session_id: SessionId,
        reason: RejectReason,
    },
    FileCancel {
        session_id: SessionId,
    },
    /// 接收方处理完全部条目后的结果。
    FileResult {
        session_id: SessionId,
        received: u32,
        failed: u32,
    },
    /// 文件连接的首帧，代替 `hello` 与配对握手。
    FileHello {
        protocol_version: u32,
        session_id: SessionId,
        token: String,
    },
}

impl Message {
    /// 文件快传的控制消息，由文件会话处理，不进协调器。
    pub fn is_file(&self) -> bool {
        matches!(
            self,
            Self::FileOffer { .. }
                | Self::FileAccept { .. }
                | Self::FileReject { .. }
                | Self::FileCancel { .. }
                | Self::FileResult { .. }
                | Self::FileHello { .. }
        )
    }
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
                platform: Platform::Macos,
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
            Message::InputCode {
                input_code: Some(InputCode(0x10)),
            },
            Message::InputCode { input_code: None },
            Message::Abort {
                handoff_id: HandoffId(7),
                reason: "timeout".into(),
            },
            Message::FileOffer {
                session_id: 9,
                summary: OfferSummary {
                    files: 2,
                    dirs: 1,
                    total_bytes: 1 << 40,
                    names: vec!["照片".into()],
                },
            },
            Message::FileAccept {
                session_id: 9,
                token: "ab".into(),
            },
            Message::FileReject {
                session_id: 9,
                reason: RejectReason::NoSpace,
            },
            Message::FileCancel { session_id: 9 },
            Message::FileResult {
                session_id: 9,
                received: 2,
                failed: 0,
            },
            Message::FileHello {
                protocol_version: PROTOCOL_VERSION,
                session_id: u64::MAX,
                token: "ab".into(),
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
            protocol_version: 6,
            platform: Platform::Macos,
            input_code: Some(InputCode(0x1B)),
            wol_mac: Some("aa:bb:cc:dd:ee:ff".parse().unwrap()),
            listen_port: 47800,
            auth: None,
        };
        assert_eq!(
            serde_json::to_string(&hello).unwrap(),
            r#"{"type":"hello","device_id":"mac","protocol_version":6,"platform":"macos","input_code":27,"wol_mac":"aa:bb:cc:dd:ee:ff","listen_port":47800}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::InputCode {
                input_code: Some(InputCode(0x10))
            })
            .unwrap(),
            r#"{"type":"input_code","input_code":16}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::InputCode { input_code: None }).unwrap(),
            r#"{"type":"input_code","input_code":null}"#
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

    #[test]
    fn file_messages_wire_format_is_stable() {
        let offer = Message::FileOffer {
            session_id: 3,
            summary: OfferSummary {
                files: 1,
                dirs: 0,
                total_bytes: 5,
                names: vec!["a.txt".into()],
            },
        };
        assert_eq!(
            serde_json::to_string(&offer).unwrap(),
            r#"{"type":"file_offer","session_id":3,"files":1,"dirs":0,"total_bytes":5,"names":["a.txt"]}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::FileReject {
                session_id: 3,
                reason: RejectReason::Declined
            })
            .unwrap(),
            r#"{"type":"file_reject","session_id":3,"reason":"declined"}"#
        );
        assert_eq!(
            serde_json::to_string(&Message::FileHello {
                protocol_version: 6,
                session_id: 3,
                token: "ff".into()
            })
            .unwrap(),
            r#"{"type":"file_hello","protocol_version":6,"session_id":3,"token":"ff"}"#
        );
        let unknown: Message =
            serde_json::from_str(r#"{"type":"file_reject","session_id":3,"reason":"later"}"#)
                .unwrap();
        assert_eq!(
            unknown,
            Message::FileReject {
                session_id: 3,
                reason: RejectReason::Error
            }
        );
    }
}
