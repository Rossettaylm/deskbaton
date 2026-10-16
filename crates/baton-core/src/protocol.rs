use serde::{Deserialize, Serialize};

use crate::ids::{DeviceId, HandoffId, InputCode, MacAddr};
use crate::state::AgentState;

pub const PROTOCOL_VERSION: u32 = 1;

/// agent 之间的消息。交棒流程见 `docs/superpowers/specs/2026-10-16-deskbaton-design.md`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    Hello {
        device_id: DeviceId,
        protocol_version: u32,
        input_code: Option<InputCode>,
        wol_mac: Option<MacAddr>,
    },
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
            },
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
    }
}
