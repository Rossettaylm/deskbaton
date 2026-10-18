//! 按对端分箱的剪贴板发件箱：在线直发，离线入箱，上线按序冲刷。

use std::collections::{HashMap, HashSet, VecDeque};

use serde::Serialize;

use crate::ids::DeviceId;
use crate::participant::Payload;
use crate::state_file::clamp_outbox_limit;

/// 已配对对端在这一轮分发时的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerRef {
    /// 发件箱键：证书指纹；没有档案时用 [`DeviceId`]。
    pub key: String,
    pub name: String,
    pub to: DeviceId,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxItem {
    pub seq: u64,
    pub format: String,
    pub data: Vec<u8>,
}

impl OutboxItem {
    pub fn to_payload(&self) -> Payload {
        Payload {
            format: self.format.clone(),
            data: self.data.clone(),
        }
    }
}

/// 给面板看的未发条目；新的在前。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutboxViewItem {
    pub seq: u64,
    /// 多于一台已配对设备时写对端名，否则为空。
    pub peer: String,
    pub format: String,
    pub text: Option<String>,
    pub png_base64: Option<String>,
}

#[derive(Debug)]
pub struct Outbox {
    limit: usize,
    next_seq: u64,
    queues: HashMap<String, VecDeque<OutboxItem>>,
    names: HashMap<String, String>,
}

impl Outbox {
    pub fn new(limit: u32) -> Self {
        Self {
            limit: clamp_outbox_limit(limit) as usize,
            next_seq: 1,
            queues: HashMap::new(),
            names: HashMap::new(),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    pub fn len(&self) -> usize {
        self.queues.values().map(VecDeque::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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
        let seq = self.next_seq;
        self.next_seq += 1;
        for peer in peers {
            self.names.insert(peer.key.clone(), peer.name.clone());
            let item = OutboxItem {
                seq,
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
            self.names.insert(peer.key.clone(), peer.name.clone());
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
        self.names.retain(|k, _| keep.contains(k));
    }

    pub fn view(&self, paired: usize) -> Vec<OutboxViewItem> {
        let show_name = paired > 1;
        let mut items: Vec<OutboxViewItem> = self
            .queues
            .iter()
            .flat_map(|(key, queue)| {
                let peer = if show_name {
                    self.names.get(key).cloned().unwrap_or_else(|| key.clone())
                } else {
                    String::new()
                };
                queue.iter().map(move |item| OutboxViewItem {
                    seq: item.seq,
                    peer: peer.clone(),
                    format: item.format.clone(),
                    text: (item.format == "text/plain")
                        .then(|| String::from_utf8_lossy(&item.data).into_owned()),
                    png_base64: (item.format == "image/png").then(|| encode_base64(&item.data)),
                })
            })
            .collect();
        items.sort_by(|a, b| b.seq.cmp(&a.seq).then(a.peer.cmp(&b.peer)));
        items
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

fn encode_base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(chunk.get(1).copied().unwrap_or(0));
        let b2 = u32::from(chunk.get(2).copied().unwrap_or(0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(char::from(T[((n >> 18) & 63) as usize]));
        out.push(char::from(T[((n >> 12) & 63) as usize]));
        if chunk.len() > 1 {
            out.push(char::from(T[((n >> 6) & 63) as usize]));
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(char::from(T[(n & 63) as usize]));
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(name: &str, online: bool) -> PeerRef {
        PeerRef {
            key: format!("fp-{name}"),
            name: name.into(),
            to: DeviceId(name.into()),
            online,
        }
    }

    fn text(s: &str) -> (String, Vec<u8>) {
        ("text/plain".into(), s.as_bytes().to_vec())
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
        let view = box_.view(1);
        let texts: Vec<_> = view.iter().filter_map(|i| i.text.clone()).collect();
        assert_eq!(texts, ["n2", "n1"]);
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
        let texts: Vec<_> = box_.view(1).iter().filter_map(|i| i.text.clone()).collect();
        assert_eq!(texts, ["n2"]);
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
        assert_eq!(box_.view(2)[0].peer, "win");
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
        let view = box_.view(2);
        assert_eq!(view.len(), 2);
        let linux = view.iter().find(|i| i.peer == "linux").unwrap();
        assert_eq!(linux.text.as_deref(), Some("a"));
        let win = view.iter().find(|i| i.peer == "win").unwrap();
        assert_eq!(win.text.as_deref(), Some("b"));
    }

    #[test]
    fn snapshot_hides_name_when_only_one_peer() {
        let mut box_ = Outbox::new(10);
        let (format, data) = text("hello");
        box_.distribute(format, data, &[peer("win", false)], |_, _| unreachable!());
        assert_eq!(box_.view(1)[0].peer, "");
        assert_eq!(box_.view(2)[0].peer, "win");
    }

    #[test]
    fn newest_first_in_snapshot() {
        let mut box_ = Outbox::new(10);
        let win = peer("win", false);
        box_.distribute(
            "text/plain".into(),
            b"old".to_vec(),
            std::slice::from_ref(&win),
            |_, _| unreachable!(),
        );
        box_.distribute(
            "text/plain".into(),
            b"new".to_vec(),
            std::slice::from_ref(&win),
            |_, _| unreachable!(),
        );
        let texts: Vec<_> = box_.view(1).iter().filter_map(|i| i.text.clone()).collect();
        assert_eq!(texts, ["new", "old"]);
    }

    #[test]
    fn encodes_base64() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
    }
}
