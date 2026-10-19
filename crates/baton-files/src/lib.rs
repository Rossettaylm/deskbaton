//! 文件快传运行时：会话表的唯一持有者，收发线程、`AwakeGuard`、对外的面板接口。
//! 设计见 `docs/superpowers/specs/2026-10-19-file-transfer-design.md`。

mod disk;
mod receive;
mod send;
mod walk;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use baton_core::event::Event;
use baton_core::files::{
    Direction, OfferSummary, Phase, RejectReason, SessionId, SessionView, Sessions, has_room,
};
use baton_core::ids::DeviceId;
use baton_core::ports::{AwakeGuard, Notifier, PeerLink, PowerControl};
use baton_core::protocol::Message;
use baton_core::{Error, Result};
use baton_net::TcpPeerLink;
use baton_net::file::{FileDownload, FileHandler};

pub use disk::downloads_dir;

/// 进度最多这么频繁地通知面板。
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// 面板：会话变化、确认模式下的新请求、收完一批文件。
pub trait FileObserver: Send + Sync {
    fn changed(&self);
    fn offered(&self, from: &DeviceId);
    /// `location` 为收下的第一个顶层条目。
    fn received(&self, from: &DeviceId, files: u32, location: Option<&Path>);
}

/// 面板上的一条传输：会话视图，加上重试和「在访达中显示」。
pub struct SessionCard {
    pub view: SessionView,
    pub can_retry: bool,
    pub reveal: Option<String>,
}

pub struct FileSettings {
    pub auto_accept: bool,
    /// `None` 表示系统「下载」文件夹。
    pub receive_dir: Option<PathBuf>,
}

type Key = (DeviceId, SessionId);

pub struct FileService {
    inner: Arc<Inner>,
}

struct Inner {
    link: Arc<TcpPeerLink>,
    power: Box<dyn PowerControl>,
    events: Mutex<Sender<Event>>,
    notifier: Arc<dyn Notifier>,
    observer: Option<Arc<dyn FileObserver>>,
    sessions: Mutex<Sessions>,
    /// 发送中会话的清单与来源，收到 `file_accept` 后交给上传线程。
    outgoing: Mutex<HashMap<Key, Vec<walk::Source>>>,
    /// 用户选中的原始路径，失败后用来重试。成功、拒绝、取消时丢掉。
    roots: Mutex<HashMap<Key, Vec<PathBuf>>>,
    cancels: Mutex<HashMap<Key, Arc<AtomicBool>>>,
    /// 释放 guard 可能执行 pmset，须在放开这把锁之后再析构。
    awake: Mutex<HashMap<Key, AwakeGuard>>,
    auto_accept: AtomicBool,
    receive_dir: Mutex<Option<PathBuf>>,
    last_changed: Mutex<Option<Instant>>,
}

impl FileService {
    /// 登记为链路的文件会话。
    pub fn start(
        link: Arc<TcpPeerLink>,
        power: Box<dyn PowerControl>,
        events: Sender<Event>,
        notifier: Arc<dyn Notifier>,
        observer: Option<Arc<dyn FileObserver>>,
        settings: FileSettings,
    ) -> Arc<Self> {
        let inner = Arc::new(Inner {
            link: Arc::clone(&link),
            power,
            events: Mutex::new(events),
            notifier,
            observer,
            sessions: Mutex::new(Sessions::default()),
            outgoing: Mutex::new(HashMap::new()),
            roots: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            awake: Mutex::new(HashMap::new()),
            auto_accept: AtomicBool::new(settings.auto_accept),
            receive_dir: Mutex::new(settings.receive_dir),
            last_changed: Mutex::new(None),
        });
        link.set_file_handler(Arc::new(Handler(Arc::clone(&inner))));
        Arc::new(Self { inner })
    }

    /// 把选中的文件和文件夹发给 `to`。对方离线时先唤醒，最多等 30 秒。
    pub fn send(&self, to: &DeviceId, paths: &[PathBuf]) -> Result<SessionId> {
        let sources = walk::collect(paths)?;
        let entries: Vec<_> = sources.iter().map(|s| s.entry.clone()).collect();
        let summary = OfferSummary::of(&entries);
        let id = random_id()?;
        let key = (to.clone(), id);
        let online = self.inner.link.is_online(to);
        lock(&self.inner.sessions).send(to, id, summary.clone(), online, Instant::now());
        lock(&self.inner.outgoing).insert(key.clone(), sources);
        lock(&self.inner.roots).insert(key.clone(), paths.to_vec());
        let cancel = self.inner.cancel_flag(&key);
        self.inner.changed();
        let inner = Arc::clone(&self.inner);
        thread::Builder::new()
            .name("file-offer".into())
            .spawn(move || inner.offer_when_online(&key, summary, &cancel))
            .map_err(|e| Error::File(format!("无法启动发送线程：{e}")))?;
        Ok(id)
    }

    pub fn accept(&self, peer: &DeviceId, id: SessionId) -> Result<()> {
        let pending = lock(&self.inner.sessions)
            .get(peer, id)
            .is_some_and(|s| s.phase == Phase::Pending);
        if !pending {
            return Err(Error::File("该请求已结束".into()));
        }
        self.inner.accept_incoming(peer, id);
        Ok(())
    }

    pub fn reject(&self, peer: &DeviceId, id: SessionId) {
        let rejected = Phase::Rejected {
            reason: RejectReason::Declined,
        };
        if self.inner.end(peer, id, rejected) {
            self.inner.send_message(
                peer,
                Message::FileReject {
                    session_id: id,
                    reason: RejectReason::Declined,
                },
            );
        }
    }

    /// 取消挂起或进行中的会话，两个方向都可以。
    pub fn cancel(&self, peer: &DeviceId, id: SessionId) {
        if self.inner.end(peer, id, Phase::Canceled { by_peer: false }) {
            self.inner
                .send_message(peer, Message::FileCancel { session_id: id });
        }
    }

    pub fn dismiss(&self, peer: &DeviceId, id: SessionId) {
        lock(&self.inner.sessions).dismiss(peer, id);
        lock(&self.inner.roots).remove(&(peer.clone(), id));
        self.inner.changed();
    }

    /// 失败的发送按原来的路径再发一次。缺了任何一个路径就拒绝，原来的卡片还在。
    pub fn retry(&self, peer: &DeviceId, id: SessionId) -> Result<SessionId> {
        let view = lock(&self.inner.sessions).get(peer, id).cloned();
        let Some(view) = view else {
            return Err(Error::File("这条发送已经结束".into()));
        };
        if view.direction != Direction::Send || !matches!(view.phase, Phase::Failed { .. }) {
            return Err(Error::File("这条发送不能重试".into()));
        }
        let paths = lock(&self.inner.roots)
            .get(&(peer.clone(), id))
            .cloned()
            .unwrap_or_default();
        let (can_retry, _) = send_failure_actions(&paths);
        if !can_retry {
            return Err(Error::File("原文件已经不在，无法重试".into()));
        }
        let new_id = self.send(peer, &paths)?;
        self.dismiss(peer, id);
        Ok(new_id)
    }

    pub fn mark_seen(&self) {
        lock(&self.inner.sessions).mark_seen();
    }

    pub fn snapshot(&self) -> Vec<SessionView> {
        let mut sessions = lock(&self.inner.sessions);
        sessions.tick(Instant::now());
        sessions.views()
    }

    /// 面板卡片：在会话视图上加上能否重试、以及「在访达中显示」的路径。
    pub fn cards(&self) -> Vec<SessionCard> {
        let views = self.snapshot();
        let roots = lock(&self.inner.roots);
        views
            .into_iter()
            .map(|view| {
                let key = (view.peer.clone(), view.id);
                let (can_retry, reveal) = match (&view.phase, view.direction) {
                    (Phase::Failed { .. }, Direction::Send) => roots
                        .get(&key)
                        .map(|paths| send_failure_actions(paths))
                        .unwrap_or((false, None)),
                    (Phase::Failed { .. }, Direction::Receive) => (false, view.location.clone()),
                    _ => (false, None),
                };
                SessionCard {
                    view,
                    can_retry,
                    reveal,
                }
            })
            .collect()
    }

    /// 本机接棒时是否该弹出面板：有挂起的请求，或最近收到、还没看过的文件。
    pub fn needs_attention(&self) -> bool {
        lock(&self.inner.sessions).needs_attention(Instant::now())
    }

    pub fn auto_accept(&self) -> bool {
        self.inner.auto_accept.load(Ordering::SeqCst)
    }

    pub fn set_auto_accept(&self, on: bool) {
        self.inner.auto_accept.store(on, Ordering::SeqCst);
    }

    /// 实际使用的接收位置。
    pub fn receive_dir(&self) -> Result<PathBuf> {
        self.inner.receive_dir()
    }

    pub fn set_receive_dir(&self, dir: Option<PathBuf>) {
        *lock(&self.inner.receive_dir) = dir;
    }

    /// 进程退出前：取消全部会话并释放保持唤醒。
    pub fn shutdown(&self) {
        for cancel in lock(&self.inner.cancels).values() {
            cancel.store(true, Ordering::SeqCst);
        }
        let guards = std::mem::take(&mut *lock(&self.inner.awake));
        drop(guards);
    }
}

impl Inner {
    fn receive_dir(&self) -> Result<PathBuf> {
        match lock(&self.receive_dir).clone() {
            Some(dir) => Ok(dir),
            None => disk::downloads_dir(),
        }
    }

    fn cancel_flag(&self, key: &Key) -> Arc<AtomicBool> {
        Arc::clone(lock(&self.cancels).entry(key.clone()).or_default())
    }

    fn is_canceled(&self, key: &Key) -> bool {
        lock(&self.cancels)
            .get(key)
            .is_none_or(|c| c.load(Ordering::SeqCst))
    }

    /// 发送线程：等目标上线（必要时先唤醒），然后发出请求。
    fn offer_when_online(&self, key: &Key, summary: OfferSummary, cancel: &AtomicBool) {
        let (peer, id) = key;
        let mut woke = false;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return;
            }
            if self.link.is_online(peer) {
                lock(&self.sessions).peer_online(peer);
                break;
            }
            if !woke {
                woke = true;
                let _ = lock(&self.events).send(Event::WakeFor(peer.clone()));
            }
            let expired = lock(&self.sessions).tick(Instant::now());
            if expired.contains(key) {
                self.notifier
                    .notify(&format!("{} 未能唤醒，未发送", peer.0));
                self.cleanup(key);
                self.changed();
                return;
            }
            thread::sleep(Duration::from_millis(200));
        }
        let offered = lock(&self.sessions)
            .get(peer, *id)
            .is_some_and(|s| s.phase == Phase::Offered);
        if !offered {
            return;
        }
        self.changed();
        if let Err(e) = self.link.send(
            peer,
            &Message::FileOffer {
                session_id: *id,
                summary,
            },
        ) {
            self.fail(key, &format!("无法联系 {}：{e}", peer.0));
        }
    }

    fn accept_incoming(&self, peer: &DeviceId, id: SessionId) {
        let result = (|| -> Result<bool> {
            let dir = self.receive_dir()?;
            std::fs::create_dir_all(&dir)
                .map_err(|e| Error::File(format!("无法创建接收位置 {}：{e}", dir.display())))?;
            let sessions = lock(&self.sessions);
            let need = sessions.get(peer, id).map_or(0, |s| s.summary.total_bytes);
            Ok(has_room(
                disk::free_space(&dir),
                need,
                sessions.reserved_bytes(),
            ))
        })();
        let room = match result {
            Ok(room) => room,
            Err(e) => {
                self.notifier
                    .notify(&format!("未能接收来自 {} 的文件：{e}", peer.0));
                self.refuse(peer, id, RejectReason::Error);
                return;
            }
        };
        if !room {
            self.notifier
                .notify(&format!("磁盘空间不足，未接收来自 {} 的文件", peer.0));
            self.refuse(peer, id, RejectReason::NoSpace);
            return;
        }
        let token = match random_token() {
            Ok(token) => token,
            Err(e) => {
                log::warn!("{e}");
                self.refuse(peer, id, RejectReason::Error);
                return;
            }
        };
        if !lock(&self.sessions).start(peer, id, Some(token.clone())) {
            return;
        }
        let key = (peer.clone(), id);
        self.cancel_flag(&key);
        self.hold_awake(&key);
        self.changed();
        self.send_message(
            peer,
            Message::FileAccept {
                session_id: id,
                token,
            },
        );
    }

    fn refuse(&self, peer: &DeviceId, id: SessionId, reason: RejectReason) {
        self.end(peer, id, Phase::Rejected { reason });
        self.send_message(
            peer,
            Message::FileReject {
                session_id: id,
                reason,
            },
        );
    }

    fn hold_awake(&self, key: &Key) {
        match self.power.stay_awake(Duration::MAX) {
            Ok(guard) => {
                lock(&self.awake).insert(key.clone(), guard);
            }
            Err(e) => log::warn!("传输期间保持唤醒失败：{e}"),
        }
    }

    /// 进入终态并清理；已结束的返回 `false`。
    fn end(&self, peer: &DeviceId, id: SessionId, phase: Phase) -> bool {
        let keep_roots = matches!(phase, Phase::Failed { .. });
        let ended = lock(&self.sessions).finish(peer, id, phase, Instant::now());
        if ended {
            if !keep_roots {
                lock(&self.roots).remove(&(peer.clone(), id));
            }
            self.cleanup(&(peer.clone(), id));
            self.changed();
        }
        ended
    }

    fn fail(&self, key: &Key, reason: &str) {
        let (peer, id) = key;
        if self.end(
            peer,
            *id,
            Phase::Failed {
                reason: reason.to_owned(),
            },
        ) {
            self.notifier.notify(reason);
        }
    }

    fn cleanup(&self, key: &Key) {
        if let Some(cancel) = lock(&self.cancels).remove(key) {
            cancel.store(true, Ordering::SeqCst);
        }
        lock(&self.outgoing).remove(key);
        let guard = lock(&self.awake).remove(key);
        drop(guard);
    }

    fn send_message(&self, peer: &DeviceId, message: Message) {
        if let Err(e) = self.link.send(peer, &message) {
            log::info!("发给 {} 的文件消息未送出：{e}", peer.0);
        }
    }

    fn changed(&self) {
        *lock(&self.last_changed) = Some(Instant::now());
        if let Some(observer) = &self.observer {
            observer.changed();
        }
    }

    /// 传输进度：限频通知面板。
    fn progress(&self, key: &Key, done: u64) {
        lock(&self.sessions).progress(&key.0, key.1, done);
        let due = lock(&self.last_changed).is_none_or(|at| at.elapsed() >= PROGRESS_EVERY);
        if due {
            self.changed();
        }
    }

    fn on_message(self: &Arc<Self>, from: &DeviceId, message: Message) {
        match message {
            Message::FileOffer {
                session_id,
                summary,
            } => self.on_offer(from, session_id, summary),
            Message::FileAccept { session_id, token } => self.on_accept(from, session_id, token),
            Message::FileReject { session_id, reason } => self.on_reject(from, session_id, reason),
            Message::FileCancel { session_id } => {
                self.end(from, session_id, Phase::Canceled { by_peer: true });
            }
            Message::FileResult {
                session_id,
                received,
                failed,
            } => self.on_result(from, session_id, received, failed),
            _ => {}
        }
    }

    fn on_accept(self: &Arc<Self>, from: &DeviceId, id: SessionId, token: String) {
        if lock(&self.sessions).start(from, id, None) {
            self.changed();
            self.spawn_upload((from.clone(), id), token);
        }
    }

    fn on_reject(&self, from: &DeviceId, id: SessionId, reason: RejectReason) {
        if self.end(from, id, Phase::Rejected { reason }) {
            self.notifier.notify(&reason.describe(from));
        }
    }

    fn on_result(&self, from: &DeviceId, id: SessionId, received: u32, failed: u32) {
        if !self.end(from, id, Phase::Done { received, failed }) {
            return;
        }
        self.notifier.notify(&match failed {
            0 => format!("已把 {received} 个文件发给 {}", from.0),
            _ => format!(
                "已送达 {received} / {} 个文件给 {}，其余失败",
                received + failed,
                from.0
            ),
        });
    }

    fn on_offer(self: &Arc<Self>, from: &DeviceId, id: SessionId, summary: OfferSummary) {
        if !lock(&self.sessions).receive(from, id, summary) {
            return;
        }
        if self.auto_accept.load(Ordering::SeqCst) {
            self.accept_incoming(from, id);
            return;
        }
        let Some(observer) = &self.observer else {
            self.refuse(from, id, RejectReason::Unavailable);
            return;
        };
        self.changed();
        observer.offered(from);
    }

    fn spawn_upload(self: &Arc<Self>, key: Key, token: String) {
        let Some(sources) = lock(&self.outgoing).remove(&key) else {
            self.fail(&key, "发送内容已丢失");
            return;
        };
        self.hold_awake(&key);
        let inner = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("file-upload".into())
            .spawn(move || send::upload(&inner, &key, &token, &sources));
        if let Err(e) = spawned {
            log::warn!("无法启动上传线程：{e}");
        }
    }

    fn on_incoming(
        self: &Arc<Self>,
        from: DeviceId,
        id: SessionId,
        token: &str,
        download: FileDownload,
    ) {
        if !lock(&self.sessions).claim_token(&from, id, token) {
            log::info!("{} 的文件连接令牌不符", from.0);
            return;
        }
        let dir = match self.receive_dir() {
            Ok(dir) => dir,
            Err(e) => {
                self.fail(&(from, id), &e.to_string());
                return;
            }
        };
        let inner = Arc::clone(self);
        let spawned = thread::Builder::new()
            .name("file-receive".into())
            .spawn(move || receive::run(&inner, &(from, id), &dir, download));
        if let Err(e) = spawned {
            log::warn!("无法启动接收线程：{e}");
        }
    }

    fn on_peer_offline(&self, peer: &DeviceId) {
        let ended = lock(&self.sessions).peer_offline(peer, Instant::now());
        for id in &ended {
            self.cleanup(&(peer.clone(), *id));
        }
        if !ended.is_empty() {
            self.notifier
                .notify(&format!("与 {} 的连接已断开，文件传输已中止", peer.0));
            self.changed();
        }
    }
}

struct Handler(Arc<Inner>);

impl FileHandler for Handler {
    fn message(&self, from: &DeviceId, message: Message) {
        self.0.on_message(from, message);
    }

    fn incoming(
        &self,
        from: DeviceId,
        session_id: SessionId,
        token: String,
        download: FileDownload,
    ) {
        self.0.on_incoming(from, session_id, &token, download);
    }

    fn peer_offline(&self, peer: &DeviceId) {
        self.0.on_peer_offline(peer);
    }
}

/// 面板以 JavaScript 数字传递会话 id，限制在 53 位以内。
fn random_id() -> Result<SessionId> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).map_err(|e| Error::File(format!("无法生成随机数：{e}")))?;
    Ok(u64::from_be_bytes(bytes) & ((1 << 53) - 1))
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| Error::File(format!("无法生成随机数：{e}")))?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 发送失败后：全部原始路径都还在才能重试；「显示」用第一个还在的路径。
fn send_failure_actions(paths: &[PathBuf]) -> (bool, Option<String>) {
    if paths.is_empty() {
        return (false, None);
    }
    let reveal = paths
        .iter()
        .find(|path| path.exists())
        .map(|path| path.display().to_string());
    let can_retry = paths.iter().all(|path| path.exists());
    (can_retry, reveal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_needs_every_original_path() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept.txt");
        let gone = dir.path().join("gone.txt");
        std::fs::write(&kept, b"a").unwrap();
        std::fs::write(&gone, b"b").unwrap();
        let (can_retry, reveal) = send_failure_actions(&[kept.clone(), gone.clone()]);
        assert!(can_retry);
        assert_eq!(reveal.as_deref(), Some(kept.to_str().unwrap()));

        std::fs::remove_file(&gone).unwrap();
        let (can_retry, reveal) = send_failure_actions(&[kept.clone(), gone]);
        assert!(!can_retry);
        assert_eq!(reveal.as_deref(), Some(kept.to_str().unwrap()));

        std::fs::remove_file(&kept).unwrap();
        let (can_retry, reveal) = send_failure_actions(std::slice::from_ref(&kept));
        assert!(!can_retry);
        assert!(reveal.is_none());
    }
}
