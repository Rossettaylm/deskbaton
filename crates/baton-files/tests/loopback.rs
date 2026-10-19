//! 两个已配对的链路在本机回环地址上互发文件；电源为假实现，接收位置为临时目录。

use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use baton_core::Result;
use baton_core::event::{Event, PeerInfo};
use baton_core::files::{
    Direction, Entry, EntryKind, OfferSummary, Phase, RejectReason, SessionView,
};
use baton_core::ids::{DeviceId, Platform};
use baton_core::ports::{AwakeGuard, Notifier, PeerLink, PowerControl};
use baton_core::protocol::Message;
use baton_files::{FileObserver, FileService, FileSettings};
use baton_net::file::{FileDownload, FileFrame, FileHandler};
use baton_net::{LinkConfig, PeerAddr, PeerStore, TcpPeerLink, TlsIdentity};

const WAIT: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
struct Guards(Arc<AtomicUsize>);

impl Guards {
    fn held(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct FakePower(Guards);

impl PowerControl for FakePower {
    fn stay_awake(&self, _: Duration) -> Result<AwakeGuard> {
        let count = Arc::clone(&(self.0).0);
        count.fetch_add(1, Ordering::SeqCst);
        Ok(AwakeGuard::new(move || {
            count.fetch_sub(1, Ordering::SeqCst);
        }))
    }
    fn wake_display(&self) -> Result<()> {
        Ok(())
    }
    fn restore_sleep(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Notes(Mutex<Vec<String>>);

impl Notifier for Notes {
    fn notify(&self, message: &str) {
        eprintln!("[notify] {message}");
        self.0.lock().unwrap().push(message.to_owned());
    }
}

#[derive(Default)]
struct Panel {
    offered: AtomicUsize,
    received: Mutex<Vec<(u32, Option<PathBuf>)>>,
}

impl FileObserver for Panel {
    fn changed(&self) {}
    fn offered(&self, _: &DeviceId) {
        self.offered.fetch_add(1, Ordering::SeqCst);
    }
    fn received(&self, _: &DeviceId, files: u32, location: Option<&Path>) {
        self.received
            .lock()
            .unwrap()
            .push((files, location.map(Path::to_path_buf)));
    }
}

fn info(name: &str) -> PeerInfo {
    PeerInfo {
        device_id: DeviceId(name.into()),
        platform: Some(Platform::current()),
        input_code: None,
        wol_mac: None,
        ip: None,
    }
}

/// 链路事件交给协调器，这里用不到，接收端直接丢弃。
fn link(name: &str, peer: Option<SocketAddr>) -> Arc<TcpPeerLink> {
    let mut config = LinkConfig::new(
        info(name),
        "127.0.0.1:0".parse().unwrap(),
        Arc::new(TlsIdentity::generate().unwrap()),
        Arc::new(PeerStore::in_memory()),
    );
    config.token = Some("token".into());
    config.peers = peer
        .map(|addr| PeerAddr {
            device_id: DeviceId("win".into()),
            address: addr.to_string(),
        })
        .into_iter()
        .collect();
    let (tx, _) = mpsc::channel();
    Arc::new(TcpPeerLink::start(config, tx).unwrap())
}

/// mac 拨号连上 win。
fn linked() -> (Arc<TcpPeerLink>, Arc<TcpPeerLink>) {
    let win = link("win", None);
    let mac = link("mac", Some(win.local_addr()));
    wait("上线", || {
        mac.is_online(&DeviceId("win".into())) && win.is_online(&DeviceId("mac".into()))
    });
    (mac, win)
}

fn wait(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !cond() {
        assert!(Instant::now() < deadline, "等待{what}超时");
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Side {
    service: Arc<FileService>,
    guards: Guards,
    notes: Arc<Notes>,
    panel: Arc<Panel>,
    inbox: tempfile::TempDir,
}

fn side(link: &Arc<TcpPeerLink>, auto_accept: bool, with_panel: bool) -> Side {
    let guards = Guards::default();
    let notes = Arc::new(Notes::default());
    let panel = Arc::new(Panel::default());
    let inbox = tempfile::tempdir().unwrap();
    let (events, _) = mpsc::channel();
    let service = FileService::start(
        Arc::clone(link),
        Box::new(FakePower(guards.clone())),
        events,
        Arc::clone(&notes) as Arc<dyn Notifier>,
        with_panel.then(|| Arc::clone(&panel) as Arc<dyn FileObserver>),
        FileSettings {
            auto_accept,
            receive_dir: Some(inbox.path().to_path_buf()),
        },
    );
    Side {
        service,
        guards,
        notes,
        panel,
        inbox,
    }
}

fn session(side: &Side, direction: Direction) -> Option<SessionView> {
    side.service
        .snapshot()
        .into_iter()
        .find(|s| s.direction == direction)
}

fn phase(side: &Side, direction: Direction) -> Option<Phase> {
    session(side, direction).map(|s| s.phase)
}

fn ended(side: &Side, direction: Direction) -> bool {
    phase(side, direction).is_some_and(|p| !p.is_active())
}

fn leftovers(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(leftovers(&path));
        } else if path.extension().is_some_and(|e| e == "part") {
            found.push(path);
        }
    }
    found
}

#[test]
fn sends_a_folder_and_files_with_rename_on_conflict() {
    let (mac, win) = linked();
    let (a, b) = (side(&mac, true, true), side(&win, true, true));
    let src = tempfile::tempdir().unwrap();
    let photos = src.path().join("照片");
    fs::create_dir_all(photos.join("空")).unwrap();
    fs::write(photos.join("1.jpg"), vec![7u8; 700_000]).unwrap();
    let note = src.path().join("note.txt");
    fs::write(&note, b"hello").unwrap();
    fs::write(b.inbox.path().join("note.txt"), b"old").unwrap();

    a.service
        .send(&DeviceId("win".into()), &[photos, note])
        .unwrap();
    wait("发送结束", || ended(&a, Direction::Send));
    assert_eq!(
        phase(&a, Direction::Send),
        Some(Phase::Done {
            received: 2,
            failed: 0
        })
    );
    wait("接收结束", || ended(&b, Direction::Receive));
    let inbox = b.inbox.path();
    assert_eq!(fs::read(inbox.join("note.txt")).unwrap(), b"old");
    assert_eq!(fs::read(inbox.join("note (1).txt")).unwrap(), b"hello");
    assert_eq!(fs::read(inbox.join("照片/1.jpg")).unwrap().len(), 700_000);
    assert!(inbox.join("照片/空").is_dir());
    assert!(leftovers(inbox).is_empty());
    assert_eq!(
        b.panel.received.lock().unwrap().as_slice(),
        [(2, Some(inbox.join("照片")))]
    );
    assert!(b.service.needs_attention(), "自动接收后接棒应提示");
    b.service.mark_seen();
    assert!(!b.service.needs_attention());
    wait("释放保持唤醒", || {
        a.guards.held() == 0 && b.guards.held() == 0
    });
}

#[test]
fn confirm_mode_waits_for_user_and_can_reject() {
    let (mac, win) = linked();
    let (a, b) = (side(&mac, true, true), side(&win, false, true));
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"abc").unwrap();
    let to_win = DeviceId("win".into());

    a.service
        .send(&to_win, std::slice::from_ref(&file))
        .unwrap();
    wait("挂起", || {
        phase(&b, Direction::Receive) == Some(Phase::Pending)
    });
    assert_eq!(b.panel.offered.load(Ordering::SeqCst), 1);
    assert_eq!(phase(&a, Direction::Send), Some(Phase::Offered));
    assert!(b.service.needs_attention());
    assert_eq!(b.guards.held(), 0, "挂起阶段不持有保持唤醒");
    let id = session(&b, Direction::Receive).unwrap().id;
    b.service.reject(&DeviceId("mac".into()), id);
    wait("被拒绝", || ended(&a, Direction::Send));
    assert_eq!(
        phase(&a, Direction::Send),
        Some(Phase::Rejected {
            reason: RejectReason::Declined
        })
    );
    assert!(a.notes.0.lock().unwrap().iter().any(|n| n.contains("拒绝")));

    let before = a.service.snapshot().len();
    a.service.send(&to_win, &[file]).unwrap();
    wait("第二次挂起", || {
        b.service
            .snapshot()
            .iter()
            .any(|s| s.phase == Phase::Pending)
    });
    let pending = b
        .service
        .snapshot()
        .into_iter()
        .find(|s| s.phase == Phase::Pending)
        .unwrap();
    b.service
        .accept(&DeviceId("mac".into()), pending.id)
        .unwrap();
    wait("送达", || {
        a.service.snapshot().len() > before
            && a.service
                .snapshot()
                .iter()
                .any(|s| matches!(s.phase, Phase::Done { received: 1, .. }))
    });
    assert_eq!(fs::read(b.inbox.path().join("a.txt")).unwrap(), b"abc");
}

#[test]
fn headless_receiver_in_confirm_mode_refuses() {
    let (mac, win) = linked();
    let (a, _b) = (side(&mac, true, true), side(&win, false, false));
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"abc").unwrap();
    a.service.send(&DeviceId("win".into()), &[file]).unwrap();
    wait("被拒绝", || ended(&a, Direction::Send));
    assert_eq!(
        phase(&a, Direction::Send),
        Some(Phase::Rejected {
            reason: RejectReason::Unavailable
        })
    );
}

#[test]
fn sender_cancel_ends_both_sides() {
    let (mac, win) = linked();
    let (a, b) = (side(&mac, true, true), side(&win, false, true));
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"abc").unwrap();
    let id = a.service.send(&DeviceId("win".into()), &[file]).unwrap();
    wait("挂起", || {
        phase(&b, Direction::Receive) == Some(Phase::Pending)
    });
    a.service.cancel(&DeviceId("win".into()), id);
    wait("对方取消", || {
        phase(&b, Direction::Receive) == Some(Phase::Canceled { by_peer: true })
    });
}

#[test]
fn offline_target_is_woken_and_send_can_be_canceled() {
    let mac = link("mac", None);
    let guards = Guards::default();
    let (events, events_rx) = mpsc::channel();
    let service = FileService::start(
        Arc::clone(&mac),
        Box::new(FakePower(guards.clone())),
        events,
        Arc::new(Notes::default()),
        None,
        FileSettings {
            auto_accept: true,
            receive_dir: None,
        },
    );
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("a.txt");
    fs::write(&file, b"abc").unwrap();
    let win = DeviceId("win".into());
    let id = service.send(&win, &[file]).unwrap();
    assert_eq!(service.snapshot()[0].phase, Phase::Waking);
    assert_eq!(
        events_rx.recv_timeout(WAIT).unwrap(),
        Event::WakeFor(win.clone())
    );
    service.cancel(&win, id);
    assert_eq!(
        service.snapshot()[0].phase,
        Phase::Canceled { by_peer: false }
    );
    assert_eq!(guards.held(), 0);
}

/// 只有链路、手动收发帧的发送方，用来构造坏数据。
#[derive(Default)]
struct RawSender(Mutex<Vec<Message>>);

impl FileHandler for RawSender {
    fn message(&self, _: &DeviceId, message: Message) {
        self.0.lock().unwrap().push(message);
    }
    fn incoming(&self, _: DeviceId, _: u64, _: String, _: FileDownload) {}
    fn peer_offline(&self, _: &DeviceId) {}
}

impl RawSender {
    fn offer(&self, link: &TcpPeerLink, total: u64) -> String {
        link.send(
            &DeviceId("win".into()),
            &Message::FileOffer {
                session_id: 42,
                summary: OfferSummary {
                    files: 1,
                    dirs: 0,
                    total_bytes: total,
                    names: Vec::new(),
                },
            },
        )
        .unwrap();
        wait("同意", || !self.0.lock().unwrap().is_empty());
        match self.0.lock().unwrap().remove(0) {
            Message::FileAccept { token, .. } => token,
            other => panic!("意外的回复：{other:?}"),
        }
    }
}

fn raw_pair() -> (Arc<TcpPeerLink>, Arc<RawSender>, Side, Arc<TcpPeerLink>) {
    let (mac, win) = linked();
    let raw = Arc::new(RawSender::default());
    mac.set_file_handler(Arc::clone(&raw) as Arc<dyn FileHandler>);
    let b = side(&win, true, true);
    (mac, raw, b, win)
}

fn file_entry(path: &[&str], size: u64) -> Entry {
    Entry {
        path: path.iter().map(|s| (*s).to_owned()).collect(),
        kind: EntryKind::File,
        size,
    }
}

#[test]
fn interrupted_upload_removes_partial_file() {
    let (mac, raw, b, _win) = raw_pair();
    let token = raw.offer(&mac, 10);
    let upload = mac
        .open_upload(&DeviceId("win".into()), 42, &token)
        .unwrap();
    upload
        .send(FileFrame::Manifest(vec![file_entry(&["big.bin"], 10)]))
        .unwrap();
    upload.send(FileFrame::Begin(0)).unwrap();
    upload.send(FileFrame::Chunk(vec![1; 4])).unwrap();
    wait("开始写入", || {
        session(&b, Direction::Receive).is_some_and(|s| s.done_bytes == 4)
    });
    assert_eq!(b.guards.held(), 1);
    upload.finish().unwrap();
    wait("失败", || ended(&b, Direction::Receive));
    assert!(matches!(
        phase(&b, Direction::Receive),
        Some(Phase::Failed { .. })
    ));
    assert!(leftovers(b.inbox.path()).is_empty());
    assert!(!b.inbox.path().join("big.bin").exists());
    assert_eq!(b.guards.held(), 0);
}

#[test]
fn bad_hash_drops_file_and_traversal_fails_session() {
    let (mac, raw, b, _win) = raw_pair();
    let token = raw.offer(&mac, 3);
    let upload = mac
        .open_upload(&DeviceId("win".into()), 42, &token)
        .unwrap();
    for frame in [
        FileFrame::Manifest(vec![file_entry(&["a.txt"], 3)]),
        FileFrame::Begin(0),
        FileFrame::Chunk(b"abc".to_vec()),
        FileFrame::End {
            sha256: Some("00".into()),
        },
        FileFrame::Done,
    ] {
        upload.send(frame).unwrap();
    }
    upload.finish().unwrap();
    wait("结束", || ended(&b, Direction::Receive));
    assert_eq!(
        phase(&b, Direction::Receive),
        Some(Phase::Done {
            received: 0,
            failed: 1
        })
    );
    assert!(!b.inbox.path().join("a.txt").exists());
    assert!(leftovers(b.inbox.path()).is_empty());

    b.service.dismiss(&DeviceId("mac".into()), 42);
    raw.0.lock().unwrap().clear();
    let token = raw.offer(&mac, 3);
    let upload = mac
        .open_upload(&DeviceId("win".into()), 42, &token)
        .unwrap();
    upload
        .send(FileFrame::Manifest(vec![file_entry(
            &["..", "evil.txt"],
            3,
        )]))
        .unwrap();
    let _ = upload.send(FileFrame::Begin(0));
    let _ = upload.finish();
    wait("判错", || ended(&b, Direction::Receive));
    assert!(matches!(
        phase(&b, Direction::Receive),
        Some(Phase::Failed { .. })
    ));
    assert!(!b.inbox.path().parent().unwrap().join("evil.txt").exists());
}

#[test]
fn file_connection_with_wrong_token_is_dropped() {
    let (mac, raw, b, _win) = raw_pair();
    let _token = raw.offer(&mac, 3);
    if let Ok(upload) = mac.open_upload(&DeviceId("win".into()), 42, "forged") {
        let _ = upload.send(FileFrame::Manifest(vec![file_entry(&["a.txt"], 3)]));
        let _ = upload.finish();
    }
    std::thread::sleep(Duration::from_millis(300));
    let s = session(&b, Direction::Receive).unwrap();
    assert_eq!(s.phase, Phase::Transferring, "伪造令牌不影响会话");
    assert_eq!(s.done_bytes, 0);
}
