//! 发送调度：控制帧永远优先；数据按传输轮转，每次只发一个块。

use std::collections::VecDeque;

use crate::frame::Frame;
use crate::transfer::OutgoingTransfer;

#[derive(Default)]
pub struct SendQueue {
    control: VecDeque<Frame>,
    transfers: VecDeque<OutgoingTransfer>,
}

impl SendQueue {
    pub fn push_control(&mut self, frame: Frame) {
        self.control.push_back(frame);
    }

    pub fn push_transfer(&mut self, transfer: OutgoingTransfer) {
        self.transfers.push_back(transfer);
    }

    pub fn is_empty(&self) -> bool {
        self.control.is_empty() && self.transfers.is_empty()
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        if let Some(frame) = self.control.pop_front() {
            return Some(frame);
        }
        while let Some(mut transfer) = self.transfers.pop_front() {
            if let Some(frame) = transfer.next_frame() {
                if !transfer.is_done() {
                    self.transfers.push_back(transfer);
                }
                return Some(frame);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use baton_core::channel::ChannelId;
    use baton_core::ids::HandoffId;
    use baton_core::participant::Payload;
    use baton_core::protocol::Message;

    use super::*;
    use crate::transfer::{CHUNK_LEN, KIND_CHUNK};

    fn transfer(id: u64, len: usize) -> OutgoingTransfer {
        let payload = Payload {
            format: "image/png".into(),
            data: vec![0; len],
        };
        OutgoingTransfer::new(ChannelId::CLIPBOARD, id, HandoffId(1), payload).unwrap()
    }

    fn heartbeat() -> Frame {
        Frame::message(&Message::Heartbeat {
            state: baton_core::state::AgentState::Idle,
        })
        .unwrap()
    }

    #[test]
    fn control_waits_at_most_one_chunk_behind_bulk_data() {
        let mut q = SendQueue::default();
        q.push_transfer(transfer(1, CHUNK_LEN * 100));
        for _ in 0..10 {
            q.next_frame().unwrap();
        }
        q.push_control(heartbeat());
        let next = q.next_frame().unwrap();
        assert_eq!(next.channel, ChannelId::CONTROL);
    }

    #[test]
    fn data_never_exceeds_one_chunk_per_frame() {
        let mut q = SendQueue::default();
        q.push_transfer(transfer(1, CHUNK_LEN * 3 + 5));
        while let Some(frame) = q.next_frame() {
            if frame.kind == KIND_CHUNK {
                assert!(frame.payload.len() <= 8 + CHUNK_LEN);
            }
        }
        assert!(q.is_empty());
    }

    #[test]
    fn transfers_take_turns() {
        let mut q = SendQueue::default();
        q.push_transfer(transfer(1, CHUNK_LEN * 4));
        q.push_transfer(transfer(2, CHUNK_LEN * 4));
        let ids: Vec<u8> = std::iter::from_fn(|| q.next_frame())
            .take(6)
            .map(|f| match f.kind {
                KIND_CHUNK => f.payload[7],
                _ => {
                    serde_json::from_slice::<crate::transfer::Begin>(&f.payload)
                        .unwrap()
                        .transfer_id as u8
                }
            })
            .collect();
        assert_eq!(ids, [1, 2, 1, 2, 1, 2]);
    }
}
