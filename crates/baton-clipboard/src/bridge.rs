use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use baton_core::Result;
use baton_core::ids::DeviceId;
use baton_core::outbox::{Outbox, PeerRef};
use baton_core::participant::Payload;
use baton_core::ports::{ClipboardAccess, ClipboardRead, Notifier};

use crate::echo::EchoSet;
use crate::sync::MAX_CLIPBOARD_BYTES;

/// 轮询系统剪贴板，按对端分发或入箱。
pub struct ClipboardBridge {
    access: Arc<dyn ClipboardAccess>,
    enabled: Arc<AtomicBool>,
    notifier: Arc<dyn Notifier>,
    echoes: Arc<Mutex<EchoSet>>,
    outbox: Mutex<Outbox>,
}

impl ClipboardBridge {
    pub fn new(
        access: Arc<dyn ClipboardAccess>,
        enabled: Arc<AtomicBool>,
        notifier: Arc<dyn Notifier>,
        echoes: Arc<Mutex<EchoSet>>,
        limit: u32,
    ) -> Self {
        Self {
            access,
            enabled,
            notifier,
            echoes,
            outbox: Mutex::new(Outbox::new(limit)),
        }
    }

    /// 丢掉已解绑的箱子；总开关打开时采集、分发、冲刷。
    pub fn tick(&self, peers: &[PeerRef], mut send: impl FnMut(&DeviceId, Payload) -> Result<()>) {
        self.outbox()
            .retain_peers(peers.iter().map(|p| p.key.as_str()));
        if !self.enabled.load(Ordering::SeqCst) {
            return;
        }
        if let Some((format, data)) = self.read_new() {
            self.echoes().remember(&format, &data);
            self.outbox()
                .distribute(format, data, peers, |to, payload| send(to, payload).is_ok());
        }
        self.outbox()
            .flush_online(peers, |to, payload| send(to, payload).is_ok());
    }

    /// 发件箱里还没送出的条数。
    pub fn pending(&self) -> usize {
        self.outbox().len()
    }

    /// 每个对端还没送出的条数。键与入箱时相同，不含正文。
    pub fn pending_counts(&self) -> Vec<(String, usize)> {
        self.outbox().counts()
    }

    fn read_new(&self) -> Option<(String, Vec<u8>)> {
        let content = match self.access.read() {
            Ok(ClipboardRead::Content(content)) => content,
            Ok(ClipboardRead::Empty | ClipboardRead::Sensitive) => return None,
            Err(e) => {
                log::warn!("读取剪贴板失败：{e}");
                return None;
            }
        };
        if content.bytes().len() > MAX_CLIPBOARD_BYTES {
            if !self.echoes().contains(content.format(), content.bytes()) {
                self.echoes().remember(content.format(), content.bytes());
                self.notifier.notify("剪贴板过大，未同步");
            }
            return None;
        }
        if self.echoes().contains(content.format(), content.bytes()) {
            return None;
        }
        Some((content.format().into(), content.bytes().to_vec()))
    }

    fn outbox(&self) -> std::sync::MutexGuard<'_, Outbox> {
        self.outbox.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn echoes(&self) -> std::sync::MutexGuard<'_, EchoSet> {
        self.echoes.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;

    use baton_core::ports::ClipboardContent;
    use baton_core::{Error, Result};

    struct FakeAccess {
        read: Mutex<ClipboardRead>,
        written: Mutex<Vec<ClipboardContent>>,
    }

    impl FakeAccess {
        fn content(content: ClipboardContent) -> Arc<Self> {
            Arc::new(Self {
                read: Mutex::new(ClipboardRead::Content(content)),
                written: Mutex::new(Vec::new()),
            })
        }

        fn empty() -> Arc<Self> {
            Arc::new(Self {
                read: Mutex::new(ClipboardRead::Empty),
                written: Mutex::new(Vec::new()),
            })
        }
    }

    impl ClipboardAccess for FakeAccess {
        fn read(&self) -> Result<ClipboardRead> {
            Ok(self.read.lock().unwrap().clone())
        }

        fn write(&self, content: &ClipboardContent) -> Result<()> {
            self.written.lock().unwrap().push(content.clone());
            Ok(())
        }
    }

    struct Quiet;

    impl Notifier for Quiet {
        fn notify(&self, _: &str) {}
    }

    struct Notes(Mutex<Vec<String>>);

    impl Notifier for Notes {
        fn notify(&self, message: &str) {
            self.0.lock().unwrap().push(message.to_owned());
        }
    }

    fn peer(name: &str, online: bool) -> PeerRef {
        PeerRef {
            key: format!("fp-{name}"),
            to: DeviceId(name.into()),
            online,
        }
    }

    fn bridge(
        access: Arc<FakeAccess>,
        enabled: bool,
        echoes: Arc<Mutex<EchoSet>>,
    ) -> ClipboardBridge {
        ClipboardBridge::new(
            access,
            Arc::new(AtomicBool::new(enabled)),
            Arc::new(Quiet),
            echoes,
            10,
        )
    }

    #[test]
    fn online_copy_sends_immediately() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let b = bridge(access, true, Arc::new(Mutex::new(EchoSet::new())));
        let sent = Mutex::new(Vec::new());
        b.tick(&[peer("win", true)], |to, payload| {
            sent.lock().unwrap().push((to.0.clone(), payload.data));
            Ok(())
        });
        assert_eq!(b.pending(), 0);
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [("win".into(), b"hello".to_vec())]
        );
    }

    #[test]
    fn offline_then_online_flushes_fifo() {
        let access = FakeAccess::content(ClipboardContent::Text("one".into()));
        let b = bridge(
            Arc::clone(&access),
            true,
            Arc::new(Mutex::new(EchoSet::new())),
        );
        b.tick(&[peer("win", false)], |_, _| unreachable!());
        *access.read.lock().unwrap() = ClipboardRead::Content(ClipboardContent::Text("two".into()));
        b.tick(&[peer("win", false)], |_, _| unreachable!());
        assert_eq!(b.pending(), 2);
        assert_eq!(b.pending_counts(), [("fp-win".into(), 2)]);

        let sent = Mutex::new(Vec::new());
        *access.read.lock().unwrap() = ClipboardRead::Empty;
        b.tick(&[peer("win", true)], |_, payload| {
            sent.lock().unwrap().push(payload.data);
            Ok(())
        });
        assert_eq!(b.pending(), 0);
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [b"one".to_vec(), b"two".to_vec()]
        );
    }

    #[test]
    fn does_not_relay_received_content() {
        let access = FakeAccess::content(ClipboardContent::Text("from-a".into()));
        let echoes = Arc::new(Mutex::new(EchoSet::new()));
        echoes.lock().unwrap().remember("text/plain", b"from-a");
        let b = bridge(access, true, echoes);
        let sent = AtomicUsize::new(0);
        b.tick(&[peer("linux", true)], |_, _| {
            sent.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(sent.load(Ordering::SeqCst), 0);
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn echo_set_blocks_flush_echo() {
        let access = FakeAccess::content(ClipboardContent::Text("one".into()));
        let echoes = Arc::new(Mutex::new(EchoSet::new()));
        let b = bridge(Arc::clone(&access), true, Arc::clone(&echoes));
        b.tick(&[peer("win", false)], |_, _| unreachable!());
        *access.read.lock().unwrap() = ClipboardRead::Content(ClipboardContent::Text("two".into()));
        b.tick(&[peer("win", false)], |_, _| unreachable!());

        echoes.lock().unwrap().remember("text/plain", b"one");
        *access.read.lock().unwrap() = ClipboardRead::Content(ClipboardContent::Text("one".into()));
        let sent = Mutex::new(Vec::new());
        b.tick(&[peer("win", true)], |_, payload| {
            sent.lock().unwrap().push(payload.data);
            Ok(())
        });
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [b"one".to_vec(), b"two".to_vec()]
        );
        b.tick(&[peer("win", true)], |_, _| {
            panic!("回声不应再发出");
        });
    }

    #[test]
    fn disabled_freezes_outbox() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let enabled = Arc::new(AtomicBool::new(false));
        let b = ClipboardBridge::new(
            access,
            Arc::clone(&enabled),
            Arc::new(Quiet),
            Arc::new(Mutex::new(EchoSet::new())),
            10,
        );
        b.tick(&[peer("win", false)], |_, _| unreachable!());
        assert_eq!(b.pending(), 0);

        enabled.store(true, Ordering::SeqCst);
        b.tick(&[peer("win", false)], |_, _| unreachable!());
        assert_eq!(b.pending(), 1);

        enabled.store(false, Ordering::SeqCst);
        b.tick(&[peer("win", true)], |_, _| {
            panic!("关掉后不得冲刷");
        });
        assert_eq!(b.pending(), 1);
    }

    #[test]
    fn unpair_drops_box() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let b = bridge(access, true, Arc::new(Mutex::new(EchoSet::new())));
        b.tick(
            &[peer("win", false), peer("linux", false)],
            |_, _| unreachable!(),
        );
        b.tick(&[peer("linux", false)], |_, _| unreachable!());
        assert_eq!(b.pending(), 1);
    }

    #[test]
    fn send_error_stays_in_box() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let b = bridge(access, true, Arc::new(Mutex::new(EchoSet::new())));
        b.tick(&[peer("win", true)], |_, _| Err(Error::Net("down".into())));
        assert_eq!(b.pending(), 1);
    }

    #[test]
    fn oversized_notifies_once() {
        let access =
            FakeAccess::content(ClipboardContent::ImagePng(vec![0; MAX_CLIPBOARD_BYTES + 1]));
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let b = ClipboardBridge::new(
            access,
            Arc::new(AtomicBool::new(true)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
            Arc::new(Mutex::new(EchoSet::new())),
            10,
        );
        b.tick(&[peer("win", true)], |_, _| unreachable!());
        b.tick(&[peer("win", true)], |_, _| unreachable!());
        assert_eq!(notes.0.lock().unwrap().as_slice(), ["剪贴板过大，未同步"]);
        assert_eq!(b.pending(), 0);
    }

    #[test]
    fn sensitive_and_empty_are_quiet() {
        let access = FakeAccess::empty();
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let b = ClipboardBridge::new(
            Arc::clone(&access) as Arc<dyn ClipboardAccess>,
            Arc::new(AtomicBool::new(true)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
            Arc::new(Mutex::new(EchoSet::new())),
            10,
        );
        *access.read.lock().unwrap() = ClipboardRead::Sensitive;
        b.tick(&[peer("win", true)], |_, _| unreachable!());
        *access.read.lock().unwrap() = ClipboardRead::Empty;
        b.tick(&[peer("win", true)], |_, _| unreachable!());
        assert!(notes.0.lock().unwrap().is_empty());
        assert_eq!(b.pending(), 0);
    }
}
