//! 数据通道的传输单元：一个 Begin 帧、若干 Chunk 帧、一个 End 帧，任一方可发 Cancel 中止。
//!
//! Begin 负载为 JSON [`Begin`]；Chunk / End / Cancel 负载以 8 字节大端 `transfer_id` 开头，
//! 同一通道上交替发送的多个传输据此区分。

use std::collections::HashMap;

use baton_core::channel::ChannelId;
use baton_core::ids::HandoffId;
use baton_core::participant::Payload;
use baton_core::{Error, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::frame::Frame;

pub const KIND_BEGIN: u8 = 0;
pub const KIND_CHUNK: u8 = 1;
pub const KIND_END: u8 = 2;
pub const KIND_CANCEL: u8 = 3;

/// 每个 Chunk 帧携带的数据量。控制帧最多等一个块。
pub const CHUNK_LEN: usize = 64 * 1024;
/// 单次传输的上限，防止对端让本机无限分配内存。
pub const MAX_TRANSFER_LEN: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Begin {
    pub transfer_id: u64,
    pub handoff_id: HandoffId,
    pub format: String,
    pub total_len: u64,
    /// 完整数据的 SHA-256（小写十六进制）。
    pub sha256: String,
}

/// 发送方：按顺序产出一次传输的全部帧。
pub struct OutgoingTransfer {
    channel: ChannelId,
    transfer_id: u64,
    begin: Option<Frame>,
    data: Vec<u8>,
    offset: usize,
    done: bool,
}

impl OutgoingTransfer {
    pub fn new(
        channel: ChannelId,
        transfer_id: u64,
        handoff_id: HandoffId,
        payload: Payload,
    ) -> Result<Self> {
        let begin = Begin {
            transfer_id,
            handoff_id,
            format: payload.format,
            total_len: payload.data.len() as u64,
            sha256: sha256_hex(&payload.data),
        };
        if begin.total_len > MAX_TRANSFER_LEN {
            return Err(Error::Net(format!("数据过大：{} 字节", begin.total_len)));
        }
        Ok(Self {
            channel,
            transfer_id,
            begin: Some(Frame {
                channel,
                kind: KIND_BEGIN,
                payload: serde_json::to_vec(&begin).map_err(|e| Error::Net(e.to_string()))?,
            }),
            data: payload.data,
            offset: 0,
            done: false,
        })
    }

    pub fn is_done(&self) -> bool {
        self.done
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        if let Some(begin) = self.begin.take() {
            return Some(begin);
        }
        if self.done {
            return None;
        }
        if self.offset < self.data.len() {
            let end = (self.offset + CHUNK_LEN).min(self.data.len());
            let mut payload = self.transfer_id.to_be_bytes().to_vec();
            payload.extend_from_slice(&self.data[self.offset..end]);
            self.offset = end;
            return Some(Frame {
                channel: self.channel,
                kind: KIND_CHUNK,
                payload,
            });
        }
        self.done = true;
        Some(Frame {
            channel: self.channel,
            kind: KIND_END,
            payload: self.transfer_id.to_be_bytes().to_vec(),
        })
    }
}

/// 一次完整、校验通过的传输。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completed {
    pub channel: ChannelId,
    pub handoff_id: HandoffId,
    pub payload: Payload,
}

struct Incoming {
    begin: Begin,
    data: Vec<u8>,
}

/// 接收方：按连接维护未完成的传输。连接断开时整体丢弃，不重传。
#[derive(Default)]
pub struct Reassembler {
    incoming: HashMap<(ChannelId, u64), Incoming>,
}

impl Reassembler {
    /// 处理一个数据通道帧。出错的传输会被丢弃，其余传输不受影响。
    pub fn accept(&mut self, frame: Frame) -> Result<Option<Completed>> {
        let channel = frame.channel;
        if frame.kind == KIND_BEGIN {
            let begin: Begin =
                serde_json::from_slice(&frame.payload).map_err(|e| Error::Net(e.to_string()))?;
            if begin.total_len > MAX_TRANSFER_LEN {
                return Err(Error::Net(format!("数据过大：{} 字节", begin.total_len)));
            }
            self.incoming.insert(
                (channel, begin.transfer_id),
                Incoming {
                    data: Vec::with_capacity(begin.total_len.min(CHUNK_LEN as u64 * 16) as usize),
                    begin,
                },
            );
            return Ok(None);
        }
        let Some((id, body)) = frame.payload.split_first_chunk::<8>() else {
            return Err(Error::Net("数据帧缺少 transfer_id".into()));
        };
        let key = (channel, u64::from_be_bytes(*id));
        match frame.kind {
            KIND_CHUNK => {
                let Some(incoming) = self.incoming.get_mut(&key) else {
                    return Err(Error::Net(format!("未知的传输 {}", key.1)));
                };
                if (incoming.data.len() + body.len()) as u64 > incoming.begin.total_len {
                    self.incoming.remove(&key);
                    return Err(Error::Net(format!("传输 {} 超出声明长度", key.1)));
                }
                incoming.data.extend_from_slice(body);
                Ok(None)
            }
            KIND_END => {
                let Some(incoming) = self.incoming.remove(&key) else {
                    return Err(Error::Net(format!("未知的传输 {}", key.1)));
                };
                if incoming.data.len() as u64 != incoming.begin.total_len
                    || sha256_hex(&incoming.data) != incoming.begin.sha256
                {
                    return Err(Error::Net(format!("传输 {} 校验失败", key.1)));
                }
                Ok(Some(Completed {
                    channel,
                    handoff_id: incoming.begin.handoff_id,
                    payload: Payload {
                        format: incoming.begin.format,
                        data: incoming.data,
                    },
                }))
            }
            KIND_CANCEL => {
                self.incoming.remove(&key);
                Ok(None)
            }
            other => Err(Error::Net(format!("未知的数据帧类型 {other}"))),
        }
    }
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(len: usize) -> Payload {
        Payload {
            format: "image/png".into(),
            data: (0..len).map(|i| (i % 251) as u8).collect(),
        }
    }

    fn frames(transfer: &mut OutgoingTransfer) -> Vec<Frame> {
        std::iter::from_fn(|| transfer.next_frame()).collect()
    }

    #[test]
    fn splits_into_begin_chunks_end() {
        let mut t = OutgoingTransfer::new(
            ChannelId::CLIPBOARD,
            7,
            HandoffId(1),
            payload(CHUNK_LEN * 2 + 1),
        )
        .unwrap();
        let kinds: Vec<u8> = frames(&mut t).iter().map(|f| f.kind).collect();
        assert_eq!(
            kinds,
            [KIND_BEGIN, KIND_CHUNK, KIND_CHUNK, KIND_CHUNK, KIND_END]
        );
        assert!(t.is_done());
    }

    #[test]
    fn reassembles_interleaved_transfers() {
        let (p1, p2) = (payload(CHUNK_LEN + 10), payload(5));
        let mut t1 =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 1, HandoffId(9), p1.clone()).unwrap();
        let mut t2 =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 2, HandoffId(9), p2.clone()).unwrap();
        let mut r = Reassembler::default();
        let mut done = Vec::new();
        loop {
            let mut progressed = false;
            for t in [&mut t1, &mut t2] {
                if let Some(frame) = t.next_frame() {
                    progressed = true;
                    done.extend(r.accept(frame).unwrap());
                }
            }
            if !progressed {
                break;
            }
        }
        let payloads: Vec<Payload> = done.into_iter().map(|c| c.payload).collect();
        assert_eq!(payloads, [p2, p1]);
    }

    #[test]
    fn empty_payload_roundtrips() {
        let mut t =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 3, HandoffId(1), payload(0)).unwrap();
        let mut r = Reassembler::default();
        let done: Vec<Completed> = frames(&mut t)
            .into_iter()
            .filter_map(|f| r.accept(f).unwrap())
            .collect();
        assert_eq!(done[0].payload, payload(0));
    }

    #[test]
    fn rejects_corrupted_data() {
        let mut t =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 4, HandoffId(1), payload(10)).unwrap();
        let mut fs = frames(&mut t);
        let last = fs[1].payload.len() - 1;
        fs[1].payload[last] ^= 0xFF;
        let mut r = Reassembler::default();
        r.accept(fs[0].clone()).unwrap();
        r.accept(fs[1].clone()).unwrap();
        assert!(r.accept(fs[2].clone()).is_err());
    }

    #[test]
    fn rejects_data_beyond_declared_length() {
        let mut r = Reassembler::default();
        let mut t =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 5, HandoffId(1), payload(4)).unwrap();
        let fs = frames(&mut t);
        r.accept(fs[0].clone()).unwrap();
        let mut oversized = fs[1].clone();
        oversized.payload.extend_from_slice(&[0; 8]);
        assert!(r.accept(oversized).is_err());
        assert!(r.accept(fs[2].clone()).is_err(), "超长后整个传输被丢弃");
    }

    #[test]
    fn cancel_drops_transfer() {
        let mut r = Reassembler::default();
        let mut t =
            OutgoingTransfer::new(ChannelId::CLIPBOARD, 6, HandoffId(1), payload(4)).unwrap();
        let fs = frames(&mut t);
        r.accept(fs[0].clone()).unwrap();
        r.accept(Frame {
            channel: ChannelId::CLIPBOARD,
            kind: KIND_CANCEL,
            payload: 6u64.to_be_bytes().to_vec(),
        })
        .unwrap();
        assert!(r.accept(fs[2].clone()).is_err());
    }
}
