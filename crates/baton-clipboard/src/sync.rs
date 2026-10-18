use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use baton_core::channel::ChannelId;
use baton_core::ids::DeviceId;
use baton_core::participant::{HandoffParticipant, Payload};
use baton_core::ports::{ClipboardAccess, ClipboardContent, Notifier};
use baton_core::{Error, Result};

use crate::echo::EchoSet;

/// 超过这个大小就不带，并提示用户。
pub const MAX_CLIPBOARD_BYTES: usize = 32 * 1024 * 1024;

/// 接收对端剪贴板并写入系统；交棒时不再打包。
pub struct ClipboardSync {
    access: Arc<dyn ClipboardAccess>,
    enabled: Arc<AtomicBool>,
    notifier: Arc<dyn Notifier>,
    echoes: Arc<Mutex<EchoSet>>,
}

impl ClipboardSync {
    pub fn new(
        access: Arc<dyn ClipboardAccess>,
        enabled: Arc<AtomicBool>,
        notifier: Arc<dyn Notifier>,
        echoes: Arc<Mutex<EchoSet>>,
    ) -> Self {
        Self {
            access,
            enabled,
            notifier,
            echoes,
        }
    }

    fn echoes(&self) -> std::sync::MutexGuard<'_, EchoSet> {
        self.echoes.lock().unwrap_or_else(PoisonError::into_inner)
    }
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
        Ok(None)
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
        self.echoes().remember(content.format(), content.bytes());
        if let Err(e) = self.access.write(&content) {
            self.notifier.notify("未能写入剪贴板");
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use baton_core::ids::DeviceId;
    use baton_core::ports::ClipboardRead;

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

    fn sync(access: Arc<FakeAccess>) -> (ClipboardSync, Arc<Notes>, Arc<Mutex<EchoSet>>) {
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let echoes = Arc::new(Mutex::new(EchoSet::new()));
        let sync = ClipboardSync::new(
            access,
            Arc::new(AtomicBool::new(true)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
            Arc::clone(&echoes),
        );
        (sync, notes, echoes)
    }

    #[test]
    fn pack_is_always_none() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let (sync, _, _) = sync(access);
        assert_eq!(sync.pack(&peer()).unwrap(), None);
    }

    #[test]
    fn disabled_does_not_unpack() {
        let access = FakeAccess::content(ClipboardContent::Text("hello".into()));
        let notes = Arc::new(Notes(Mutex::new(Vec::new())));
        let sync = ClipboardSync::new(
            Arc::clone(&access) as Arc<dyn ClipboardAccess>,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&notes) as Arc<dyn Notifier>,
            Arc::new(Mutex::new(EchoSet::new())),
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
    fn unpack_writes_and_remembers_echo() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, _, echoes) = sync(Arc::clone(&access));
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
        assert!(echoes.lock().unwrap().contains("text/plain", b"from-peer"));
    }

    #[test]
    fn unpack_ignores_unknown_format() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, notes, _) = sync(Arc::clone(&access));
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

    #[test]
    fn unpack_skips_oversized_and_notifies() {
        let access = FakeAccess::read(ClipboardRead::Empty);
        let (sync, notes, _) = sync(Arc::clone(&access));
        sync.unpack(
            &peer(),
            Payload {
                format: "image/png".into(),
                data: vec![0; MAX_CLIPBOARD_BYTES + 1],
            },
        )
        .unwrap();
        assert!(access.written.lock().unwrap().is_empty());
        assert_eq!(notes.0.lock().unwrap().as_slice(), ["剪贴板过大，未同步"]);
    }
}
