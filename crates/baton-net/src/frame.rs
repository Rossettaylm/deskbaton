//! 帧格式：4 字节大端长度 + JSON 消息体。

use baton_core::protocol::Message;
use baton_core::{Error, Result};

pub const MAX_FRAME_LEN: usize = 1 << 20;
const HEADER_LEN: usize = 4;

pub fn encode(msg: &Message) -> Result<Vec<u8>> {
    let body = serde_json::to_vec(msg).map_err(|e| Error::Net(e.to_string()))?;
    if body.len() > MAX_FRAME_LEN {
        return Err(Error::Net(format!("消息过大：{} 字节", body.len())));
    }
    let len = u32::try_from(body.len()).map_err(|e| Error::Net(e.to_string()))?;
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// 从缓冲区头部解出一帧。数据不足时返回 `Ok(None)`；成功时返回消息与消耗的字节数。
pub fn decode(buf: &[u8]) -> Result<Option<(Message, usize)>> {
    let Some((header, rest)) = buf.split_first_chunk::<HEADER_LEN>() else {
        return Ok(None);
    };
    let len = u32::from_be_bytes(*header) as usize;
    if len > MAX_FRAME_LEN {
        return Err(Error::Net(format!("帧长度非法：{len} 字节")));
    }
    let Some(body) = rest.get(..len) else {
        return Ok(None);
    };
    let msg = serde_json::from_slice(body).map_err(|e| Error::Net(e.to_string()))?;
    Ok(Some((msg, HEADER_LEN + len)))
}

#[cfg(test)]
mod tests {
    use baton_core::ids::HandoffId;

    use super::*;

    #[test]
    fn roundtrip_and_partial_reads() {
        let msg = Message::Taken {
            handoff_id: HandoffId(42),
        };
        let mut buf = encode(&msg).unwrap();
        let frame_len = buf.len();
        buf.extend(encode(&msg).unwrap());

        assert_eq!(decode(&buf[..3]).unwrap(), None);
        assert_eq!(decode(&buf[..frame_len - 1]).unwrap(), None);

        let (decoded, used) = decode(&buf).unwrap().unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(used, frame_len);
    }

    #[test]
    fn rejects_oversized_length() {
        let header = u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes();
        assert!(decode(&header).is_err());
    }
}
