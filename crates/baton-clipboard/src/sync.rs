use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use baton_core::channel::ChannelId;
use baton_core::ids::DeviceId;
use baton_core::participant::{HandoffParticipant, Payload};
use baton_core::ports::{ClipboardAccess, ClipboardContent, ClipboardRead, Notifier};
use baton_core::{Error, Result};
use sha2::{Digest, Sha256};

/// 超过这个大小就不带，并提示用户。
pub const MAX_CLIPBOARD_BYTES: usize = 32 * 1024 * 1024;

/// 交棒时把系统剪贴板带给接棒方。
pub struct ClipboardSync {
    access: Arc<dyn ClipboardAccess>,
    enabled: Arc<AtomicBool>,
    notifier: Arc<dyn Notifier>,
    last_hash: Mutex<Option<[u8; 32]>>,
}

impl ClipboardSync {
    pub fn new(
        access: Arc<dyn ClipboardAccess>,
        enabled: Arc<AtomicBool>,
        notifier: Arc<dyn Notifier>,
    ) -> Self {
        Self {
            access,
            enabled,
            notifier,
            last_hash: Mutex::new(None),
        }
    }

    fn remember(&self, format: &str, data: &[u8]) {
        *self.last_hash() = Some(content_hash(format, data));
    }

    fn already_sent(&self, format: &str, data: &[u8]) -> bool {
        *self.last_hash() == Some(content_hash(format, data))
    }

    fn last_hash(&self) -> std::sync::MutexGuard<'_, Option<[u8; 32]>> {
        self.last_hash
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn content_hash(format: &str, data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(format.as_bytes());
    hasher.update([0]);
    hasher.update(data);
    hasher.finalize().into()
}

fn decode(payload: Payload) -> Result<Option<ClipboardContent>> {
    match payload.format.as_str() {
        "text/plain" => {
            let text = String::from_utf8(payload.data)
                .map_err(|e| Error::Clipboard(format!("文本不是 UTF-8：{e}")))?;
            Ok(Some(ClipboardContent::Text(text)))
        }
        "image/png" => Ok(Some(ClipboardContent::ImagePng(payload.data))),
        other => {
            log::warn!("不认识的剪贴板格式：{other}");
            Ok(None)
        }
    }
}

impl HandoffParticipant for ClipboardSync {
    fn channel(&self) -> ChannelId {
        ChannelId::CLIPBOARD
    }

    fn pack(&self, _: &DeviceId) -> Result<Option<Payload>> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let content = match self.access.read() {
            Ok(ClipboardRead::Content(content)) => content,
            Ok(ClipboardRead::Empty | ClipboardRead::Sensitive) => return Ok(None),
            Err(e) => {
                log::warn!("读取剪贴板失败：{e}");
                return Ok(None);
            }
        };
        if content.bytes().len() > MAX_CLIPBOARD_BYTES {
            self.notifier.notify("剪贴板过大，未同步");
            return Ok(None);
        }
        if self.already_sent(content.format(), content.bytes()) {
            return Ok(None);
        }
        self.remember(content.format(), content.bytes());
        Ok(Some(Payload {
            format: content.format().into(),
            data: content.bytes().to_vec(),
        }))
    }

    fn unpack(&self, _: &DeviceId, payload: Payload) -> Result<()> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Ok(());
        }
        let Some(content) = decode(payload)? else {
            return Ok(());
        };
        if content.bytes().len() > MAX_CLIPBOARD_BYTES {
            self.notifier.notify("剪贴板过大，未同步");
            return Ok(());
        }
        if let Err(e) = self.access.write(&content) {
            self.notifier.notify("未能写入剪贴板");
            return Err(e);
        }
        self.remember(content.format(), content.bytes());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use baton_core::ids::DeviceId;

    struct FakeAccess {
        read: Mutex<ClipboardRead>,
        written: Mutex<Vec<ClipboardContent>>,
    }

    impl FakeAccess {
        fn content(content: ClipboardContent) -> Arc<Self> {
            Self::read(ClipboardRead::Content(content))
        }

        fn read(read: ClipboardRead) -> Arc<Self> {
            Arc::new(Self {
                read: Mutex::new(read),
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

    struct Notes(Mutex<Vec<String>>);

    impl Notifier for Notes {
        fn notify(&self, message: &str) {
            self.0.lock().unwrap().push(message.to_owned());
        }
    }

    fn peer() -> DeviceId {
        DeviceId("win".into())
    }

    fn sync(access: Arc<FakeAccess>) -> (ClipboardSync, Arc<Notes>) {
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let sync = ClipboardSync::new(
            access,
            Arc::new(AtomicBool::new(true)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
        );
        (sync, notes)
    }

    #[test]
    fn packs_text() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let (sync, _) = sync(access);
        let payload = sync.pack(&peer()).unwrap().unwrap();
        assert_eq!(payload.format, "text/plain");
        assert_eq!(payload.data, b"hello");
    }

    #[test]
    fn packs_png() {
        let access = FakeAccess::content(ClipboardContent::ImagePng(vec![1, 2, 3]));
        let (sync, _) = sync(access);
        let payload = sync.pack(&peer()).unwrap().unwrap();
        assert_eq!(payload.format, "image/png");
        assert_eq!(payload.data, [1, 2, 3]);
    }

    #[test]
    fn skips_unchanged_after_pack() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let (sync, _) = sync(access);
        assert!(sync.pack(&peer()).unwrap().is_some());
        assert_eq!(sync.pack(&peer()).unwrap(), None);
    }

    #[test]
    fn skips_oversized_and_notifies() {
        let access =
            FakeAccess::content(ClipboardContent::ImagePng(vec![0; MAX_CLIPBOARD_BYTES + 1]));
        let (sync, notes) = sync(access);
        assert_eq!(sync.pack(&peer()).unwrap(), None);
        assert_eq!(notes.0.lock().unwrap().as_slice(), ["剪贴板过大，未同步"]);
    }

    #[test]
    fn skips_sensitive_quietly() {
        let access = FakeAccess::read(ClipboardRead::Sensitive);
        let (sync, notes) = sync(access);
        assert_eq!(sync.pack(&peer()).unwrap(), None);
        assert!(notes.0.lock().unwrap().is_empty());
    }

    #[test]
    fn skips_empty() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, notes) = sync(access);
        assert_eq!(sync.pack(&peer()).unwrap(), None);
        assert!(notes.0.lock().unwrap().is_empty());
    }

    #[test]
    fn disabled_does_not_pack_or_unpack() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let sync = ClipboardSync::new(
            Arc::clone(&access) as Arc<dyn ClipboardAccess>,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
        );
        assert_eq!(sync.pack(&peer()).unwrap(), None);
        sync.unpack(
            &peer(),
            Payload {
                format: "text/plain".into(),
                data: b"from-peer".to_vec(),
            },
        )
        .unwrap();
        assert!(access.written.lock().unwrap().is_empty());
    }

    #[test]
    fn unpack_writes_and_prevents_echo() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, _) = sync(Arc::clone(&access));
        sync.unpack(
            &peer(),
            Payload {
                format: "text/plain".into(),
                data: b"from-peer".to_vec(),
            },
        )
        .unwrap();
        assert_eq!(
            access.written.lock().unwrap().as_slice(),
            [ClipboardContent::Text("from-peer".into())]
        );
        *access.read.lock().unwrap() =
            ClipboardRead::Content(ClipboardContent::Text("from-peer".into()));
        assert_eq!(sync.pack(&peer()).unwrap(), None);
    }

    #[test]
    fn unpack_ignores_unknown_format() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, notes) = sync(Arc::clone(&access));
        sync.unpack(
            &peer(),
            Payload {
                format: "video/mp4".into(),
                data: vec![0, 1],
            },
        )
        .unwrap();
        assert!(access.written.lock().unwrap().is_empty());
        assert!(notes.0.lock().unwrap().is_empty());
    }
}
