//! 帧格式：`len: u32 BE | channel: u8 | kind: u8 | payload`，`len` 为 `2 + payload` 的字节数。

use baton_core::channel::ChannelId;
use baton_core::protocol::Message;
use baton_core::{Error, Result};

/// 单帧 `len` 的上限。
pub const MAX_FRAME_LEN: usize = 1 << 20;
const LEN_BYTES: usize = 4;
const HEADER_AFTER_LEN: usize = 2;

/// 控制通道上唯一的 kind：JSON [`Message`]。
pub const KIND_MESSAGE: u8 = 0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: ChannelId,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn message(msg: &Message) -> Result<Self> {
        Ok(Self {
            channel: ChannelId::CONTROL,
            kind: KIND_MESSAGE,
            payload: serde_json::to_vec(msg).map_err(|e| Error::Net(e.to_string()))?,
        })
    }

    pub fn to_message(&self) -> Result<Message> {
        if self.channel != ChannelId::CONTROL || self.kind != KIND_MESSAGE {
            return Err(Error::Net(format!(
                "不是控制消息帧：通道 {} 类型 {}",
                self.channel.0, self.kind
            )));
        }
        serde_json::from_slice(&self.payload).map_err(|e| Error::Net(e.to_string()))
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        let len = HEADER_AFTER_LEN + self.payload.len();
        if len > MAX_FRAME_LEN {
            return Err(Error::Net(format!("帧过大：{len} 字节")));
        }
        let mut out = Vec::with_capacity(LEN_BYTES + len);
        out.extend_from_slice(&(len as u32).to_be_bytes());
        out.push(self.channel.0);
        out.push(self.kind);
        out.extend_from_slice(&self.payload);
        Ok(out)
    }
}

/// 从缓冲区头部解出一帧。数据不足时返回 `Ok(None)`；成功时返回帧与消耗的字节数。
pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>> {
    let Some((len, rest)) = buf.split_first_chunk::<LEN_BYTES>() else {
        return Ok(None);
    };
    let len = u32::from_be_bytes(*len) as usize;
    if !(HEADER_AFTER_LEN..=MAX_FRAME_LEN).contains(&len) {
        return Err(Error::Net(format!("帧长度非法：{len} 字节")));
    }
    let Some(body) = rest.get(..len) else {
        return Ok(None);
    };
    let frame = Frame {
        channel: ChannelId(body[0]),
        kind: body[1],
        payload: body[HEADER_AFTER_LEN..].to_vec(),
    };
    Ok(Some((frame, LEN_BYTES + len)))
}

#[cfg(test)]
mod tests {
    use baton_core::ids::HandoffId;

    use super::*;

    #[test]
    fn wire_format_is_stable() {
        let frame = Frame::message(&Message::Taken {
            handoff_id: HandoffId(1),
        })
        .unwrap();
        let body = br#"{"type":"taken","handoff_id":1}"#;
        let mut expected = ((2 + body.len()) as u32).to_be_bytes().to_vec();
        expected.extend_from_slice(&[0, 0]);
        expected.extend_from_slice(body);
        assert_eq!(frame.encode().unwrap(), expected);
    }

    #[test]
    fn roundtrip_and_partial_reads() {
        let frame = Frame {
            channel: ChannelId::CLIPBOARD,
            kind: 1,
            payload: vec![1, 2, 3],
        };
        let mut buf = frame.encode().unwrap();
        let frame_len = buf.len();
        buf.extend(frame.encode().unwrap());

        assert_eq!(decode(&buf[..3]).unwrap(), None);
        assert_eq!(decode(&buf[..frame_len - 1]).unwrap(), None);
        assert_eq!(decode(&buf).unwrap(), Some((frame, frame_len)));
    }

    #[test]
    fn rejects_illegal_length() {
        let too_big = ((MAX_FRAME_LEN + 1) as u32).to_be_bytes();
        assert!(decode(&too_big).is_err());
        assert!(decode(&1u32.to_be_bytes()).is_err());
    }

    #[test]
    fn message_frames_roundtrip() {
        let msg = Message::Prepare {
            handoff_id: HandoffId(5),
        };
        assert_eq!(Frame::message(&msg).unwrap().to_message().unwrap(), msg);
    }
}
