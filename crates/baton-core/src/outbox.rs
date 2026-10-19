//! 按对端分箱的剪贴板发件箱：在线直发，离线入箱，上线按序冲刷。

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ids::DeviceId;
use crate::participant::Payload;
use crate::state_file::clamp_outbox_limit;

/// 已配对对端在这一轮分发时的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRef {
    /// 发件箱键：证书指纹；没有档案时用 [`DeviceId`]。
    pub key: String,
    pub to: DeviceId,
    pub online: bool,
}

#[derive(Debug)]
struct OutboxItem {
    format: String,
    data: Vec<u8>,
}

impl OutboxItem {
    fn to_payload(&self) -> Payload {
        Payload {
            format: self.format.clone(),
            data: self.data.clone(),
        }
    }
}

#[derive(Debug)]
pub struct Outbox {
    limit: usize,
    queues: HashMap<String, VecDeque<OutboxItem>>,
}

impl Outbox {
    pub fn new(limit: u32) -> Self {
        Self {
            limit: clamp_outbox_limit(limit) as usize,
            queues: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 每个还有内容的发件箱的条数。键是对端指纹，不含正文。
    pub fn counts(&self) -> Vec<(String, usize)> {
        let mut counts: Vec<_> = self
            .queues
            .iter()
            .filter(|(_, queue)| !queue.is_empty())
            .map(|(key, queue)| (key.clone(), queue.len()))
            .collect();
        counts.sort_by(|a, b| a.0.cmp(&b.0));
        counts
    }

    pub fn set_limit(&mut self, limit: u32) {
        self.limit = clamp_outbox_limit(limit) as usize;
        for queue in self.queues.values_mut() {
            while queue.len() > self.limit {
                queue.pop_front();
            }
        }
        self.queues.retain(|_, q| !q.is_empty());
    }

    /// 本机新复制：在线且 `send` 成功则不入箱，否则追加到该对端箱尾。
    pub fn distribute(
        &mut self,
        format: String,
        data: Vec<u8>,
        peers: &[PeerRef],
        mut send: impl FnMut(&DeviceId, Payload) -> bool,
    ) {
        for peer in peers {
            let item = OutboxItem {
                format: format.clone(),
                data: data.clone(),
            };
            if peer.online && send(&peer.to, item.to_payload()) {
                continue;
            }
            self.push(&peer.key, item);
        }
    }

    /// 只冲刷当前在线的对端；入队失败则停下，留着等下次。
    pub fn flush_online(
        &mut self,
        peers: &[PeerRef],
        mut send: impl FnMut(&DeviceId, Payload) -> bool,
    ) {
        for peer in peers {
            if !peer.online {
                continue;
            }
            while let Some(item) = self.front(&peer.key) {
                if !send(&peer.to, item.to_payload()) {
                    break;
                }
                self.pop_front(&peer.key);
            }
        }
    }

    /// 解绑或不在档案里的对端丢掉箱子。
    pub fn retain_peers(&mut self, keys: impl IntoIterator<Item = impl AsRef<str>>) {
        let keep: HashSet<String> = keys.into_iter().map(|k| k.as_ref().to_owned()).collect();
        self.queues.retain(|k, _| keep.contains(k));
    }

    fn push(&mut self, key: &str, item: OutboxItem) {
        let queue = self.queues.entry(key.to_owned()).or_default();
        queue.push_back(item);
        while queue.len() > self.limit {
            queue.pop_front();
        }
    }

    fn front(&self, key: &str) -> Option<&OutboxItem> {
        self.queues.get(key).and_then(|q| q.front())
    }

    fn pop_front(&mut self, key: &str) {
        if let Some(queue) = self.queues.get_mut(key) {
            queue.pop_front();
            if queue.is_empty() {
                self.queues.remove(key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(name: &str, online: bool) -> PeerRef {
        PeerRef {
            key: format!("fp-{name}"),
            to: DeviceId(name.into()),
            online,
        }
    }

    fn text(s: &str) -> (String, Vec<u8>) {
        ("text/plain".into(), s.as_bytes().to_vec())
    }

    fn queued_texts(box_: &Outbox, key: &str) -> Vec<String> {
        box_.queues
            .get(key)
            .map(|q| {
                q.iter()
                    .map(|i| String::from_utf8_lossy(&i.data).into_owned())
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn online_send_skips_outbox() {
        let mut box_ = Outbox::new(10);
        let mut sent = Vec::new();
        let (format, data) = text("hello");
        box_.distribute(format, data, &[peer("win", true)], |to, payload| {
            sent.push((to.0.clone(), payload.data));
            true
        });
        assert!(box_.is_empty());
        assert_eq!(sent, [("win".into(), b"hello".to_vec())]);
    }

    #[test]
    fn offline_enqueues_and_flush_preserves_order() {
        let mut box_ = Outbox::new(10);
        let win = peer("win", false);
        let (f1, d1) = text("one");
        let (f2, d2) = text("two");
        box_.distribute(f1, d1, std::slice::from_ref(&win), |_, _| unreachable!());
        box_.distribute(f2, d2, std::slice::from_ref(&win), |_, _| unreachable!());
        assert_eq!(box_.len(), 2);
        assert_eq!(box_.counts(), [("fp-win".into(), 2)]);

        let mut sent = Vec::new();
        let online = peer("win", true);
        box_.flush_online(&[online], |to, payload| {
            sent.push((to.0.clone(), payload.data));
            true
        });
        assert!(box_.is_empty());
        assert_eq!(
            sent,
            [
                ("win".into(), b"one".to_vec()),
                ("win".into(), b"two".to_vec())
            ]
        );
    }

    #[test]
    fn send_failure_keeps_item() {
        let mut box_ = Outbox::new(10);
        let (format, data) = text("hello");
        box_.distribute(format, data, &[peer("win", true)], |_, _| false);
        assert_eq!(box_.len(), 1);
        let mut attempts = 0;
        box_.flush_online(&[peer("win", true)], |_, _| {
            attempts += 1;
            false
        });
        assert_eq!(attempts, 1);
        assert_eq!(box_.len(), 1);
    }

    #[test]
    fn over_limit_drops_oldest() {
        let mut box_ = Outbox::new(2);
        let win = peer("win", false);
        for i in 0..3 {
            let (format, data) = text(&format!("n{i}"));
            box_.distribute(
                format,
                data,
                std::slice::from_ref(&win),
                |_, _| unreachable!(),
            );
        }
        assert_eq!(queued_texts(&box_, "fp-win"), ["n1", "n2"]);
    }

    #[test]
    fn shrinking_limit_trims_oldest() {
        let mut box_ = Outbox::new(3);
        let win = peer("win", false);
        for i in 0..3 {
            let (format, data) = text(&format!("n{i}"));
            box_.distribute(
                format,
                data,
                std::slice::from_ref(&win),
                |_, _| unreachable!(),
            );
        }
        box_.set_limit(1);
        assert_eq!(queued_texts(&box_, "fp-win"), ["n2"]);
    }

    #[test]
    fn unpair_drops_that_box_only() {
        let mut box_ = Outbox::new(10);
        let (format, data) = text("hello");
        box_.distribute(
            format,
            data,
            &[peer("win", false), peer("linux", false)],
            |_, _| unreachable!(),
        );
        box_.retain_peers(["fp-win"]);
        assert_eq!(queued_texts(&box_, "fp-win"), ["hello"]);
        assert!(queued_texts(&box_, "fp-linux").is_empty());
        assert_eq!(box_.len(), 1);
    }

    #[test]
    fn per_peer_boxes_do_not_share_overflow() {
        let mut box_ = Outbox::new(1);
        box_.distribute(
            "text/plain".into(),
            b"a".to_vec(),
            &[peer("win", false), peer("linux", false)],
            |_, _| unreachable!(),
        );
        box_.distribute(
            "text/plain".into(),
            b"b".to_vec(),
            &[peer("win", false)],
            |_, _| unreachable!(),
        );
        assert_eq!(box_.len(), 2);
        assert_eq!(queued_texts(&box_, "fp-linux"), ["a"]);
        assert_eq!(queued_texts(&box_, "fp-win"), ["b"]);
    }
}
