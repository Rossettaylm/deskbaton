use std::collections::VecDeque;

use sha2::{Digest, Sha256};

const CAP: usize = 32;

/// 最近发出和写入的内容哈希，挡住回传与冲刷引起的回声。
#[derive(Debug, Default)]
pub struct EchoSet {
    hashes: VecDeque<[u8; 32]>,
}

impl EchoSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn remember(&mut self, format: &str, data: &[u8]) {
        let hash = content_hash(format, data);
        if self.hashes.iter().any(|h| *h == hash) {
            return;
        }
        self.hashes.push_back(hash);
        while self.hashes.len() > CAP {
            self.hashes.pop_front();
        }
    }

    pub fn contains(&self, format: &str, data: &[u8]) -> bool {
        let hash = content_hash(format, data);
        self.hashes.iter().any(|h| *h == hash)
    }
}

fn content_hash(format: &str, data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(format.as_bytes());
    hasher.update([0]);
    hasher.update(data);
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_and_detects() {
        let mut set = EchoSet::new();
        set.remember("text/plain", b"hello");
        assert!(set.contains("text/plain", b"hello"));
        assert!(!set.contains("text/plain", b"other"));
        assert!(!set.contains("image/png", b"hello"));
    }

    #[test]
    fn drops_oldest_after_cap() {
        let mut set = EchoSet::new();
        for i in 0..=CAP {
            set.remember("text/plain", &[i as u8]);
        }
        assert!(!set.contains("text/plain", &[0]));
        assert!(set.contains("text/plain", &[CAP as u8]));
        assert_eq!(set.hashes.len(), CAP);
    }

    #[test]
    fn flush_echo_not_just_last() {
        let mut set = EchoSet::new();
        set.remember("text/plain", b"one");
        set.remember("text/plain", b"two");
        assert!(set.contains("text/plain", b"one"));
        assert!(set.contains("text/plain", b"two"));
    }
}
