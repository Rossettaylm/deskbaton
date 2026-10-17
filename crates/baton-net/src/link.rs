//! TCP + TLS 链路：每对 agent 一条连接，控制帧优先；握手、配对、心跳与离线判定都在这里维护。

mod dial;
mod handshake;

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use baton_core::channel::ChannelId;
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::{DeviceId, HandoffId};
use baton_core::participant::Payload;
use baton_core::ports::PeerLink;
use baton_core::protocol::Message;
use baton_core::state::AgentState;
use baton_core::{Error, Result};
use rustls::pki_types::CertificateDer;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Notify, oneshot, watch};
use tokio::time::{MissedTickBehavior, timeout};
use tokio_rustls::TlsAcceptor;

use crate::auth::{self, Fingerprint, Role};
use crate::frame::{self, Frame};
use crate::identity::TlsIdentity;
use crate::mux::SendQueue;
use crate::store::{PairedPeer, PeerStore};
use crate::transfer::{OutgoingTransfer, Reassembler};

use handshake::Intent;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// 等待用户核对配对码的时长。
const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REDIAL_INTERVAL: Duration = Duration::from_secs(2);

/// 配置里写了地址的对端：对端声明的名称必须与 `device_id` 一致。
#[derive(Debug, Clone)]
pub struct PeerAddr {
    pub device_id: DeviceId,
    /// `host:port`。
    pub address: String,
}

/// 配对界面。提供它的链路才会弹出配对请求；没有时未配对的设备一律拒绝。
pub trait LinkObserver: Send + Sync {
    /// 设备、配对会话或连接状态变了，调用 [`TcpPeerLink::snapshot`] 取最新状态。
    fn changed(&self);
    /// 一次性提示，如「已与 X 配对」。
    fn notice(&self, message: &str);
}

#[derive(Clone)]
pub struct LinkConfig {
    pub me: PeerInfo,
    pub listen: SocketAddr,
    pub identity: Arc<TlsIdentity>,
    pub store: Arc<PeerStore>,
    /// 配置文件里的对端：其地址作为兜底；有令牌时也会主动拨号未配对的这些对端。
    pub peers: Vec<PeerAddr>,
    pub token: Option<String>,
    pub observer: Option<Arc<dyn LinkObserver>>,
    pub heartbeat_interval: Duration,
    /// 连续这么多个心跳周期收不到任何帧即判定离线。
    pub missed_heartbeats: u32,
}

impl LinkConfig {
    pub fn new(
        me: PeerInfo,
        listen: SocketAddr,
        identity: Arc<TlsIdentity>,
        store: Arc<PeerStore>,
    ) -> Self {
        Self {
            me,
            listen,
            identity,
            store,
            peers: Vec::new(),
            token: None,
            observer: None,
            heartbeat_interval: Duration::from_secs(2),
            missed_heartbeats: 3,
        }
    }
}

/// 给配对界面的状态快照。
#[derive(Debug, Clone, Serialize)]
pub struct LinkSnapshot {
    pub me: DeviceId,
    pub fingerprint: String,
    pub paired: Vec<PairedStatus>,
    pub sessions: Vec<PairingSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PairedStatus {
    pub peer: PairedPeer,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PairingSession {
    pub id: u64,
    pub peer: DeviceId,
    pub fingerprint: String,
    pub code: String,
    /// 需要本机用户同意；为 false 时本机已信任对方，只显示配对码供对方核对。
    pub awaiting_me: bool,
}

pub struct TcpPeerLink {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    handle: Handle,
    runtime: Option<Runtime>,
}

impl TcpPeerLink {
    /// 开始监听并拨号已知的对端；收到的一切以 [`Event`] 发往 `events`。
    pub fn start(config: LinkConfig, events: mpsc::Sender<Event>) -> Result<Self> {
        let token = match config.token {
            Some(token) if token.is_empty() => {
                return Err(Error::Config("network.token 不能为空字符串".into()));
            }
            token => token.map(String::into_bytes),
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("deskbaton-net")
            .enable_all()
            .build()
            .map_err(|e| Error::Net(format!("启动网络线程失败：{e}")))?;
        let listener = runtime
            .block_on(TcpListener::bind(config.listen))
            .map_err(|e| Error::Net(format!("监听 {} 失败：{e}", config.listen)))?;
        let local_addr = listener.local_addr().map_err(io)?;
        let interval = config.heartbeat_interval;
        let shared = Arc::new(Shared {
            me: config.me,
            listen_port: local_addr.port(),
            token,
            heartbeat: interval,
            offline_after: interval * config.missed_heartbeats.max(1),
            identity: config.identity,
            store: config.store,
            config_peers: config.peers,
            observer: config.observer,
            events,
            state: Mutex::new(AgentState::Idle),
            conns: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            rejected: Mutex::new(HashSet::new()),
            dialing: Mutex::new(HashSet::new()),
            next_generation: AtomicU64::new(0),
            next_transfer: AtomicU64::new(0),
            next_session: AtomicU64::new(1),
        });
        runtime.spawn(accept_loop(Arc::clone(&shared), listener));
        runtime.spawn(dial::supervise(Arc::clone(&shared)));
        Ok(Self {
            shared,
            local_addr,
            handle: runtime.handle().clone(),
            runtime: Some(runtime),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    pub fn snapshot(&self) -> LinkSnapshot {
        let conns = lock(&self.shared.conns);
        let paired = self
            .shared
            .store
            .list()
            .into_iter()
            .map(|peer| PairedStatus {
                online: conns
                    .get(&peer.device_id())
                    .is_some_and(|c| peer.matches(&c.fingerprint)),
                peer,
            })
            .collect();
        drop(conns);
        let mut sessions: Vec<PairingSession> = lock(&self.shared.sessions)
            .values()
            .map(|s| s.view.clone())
            .collect();
        sessions.sort_by_key(|s| s.id);
        LinkSnapshot {
            me: self.shared.me.device_id.clone(),
            fingerprint: auth::to_hex(&self.shared.identity.fingerprint),
            paired,
            sessions,
        }
    }

    /// 用户对配对请求的决定。
    pub fn confirm_pairing(&self, session: u64, accept: bool) -> Result<()> {
        let decide = lock(&self.shared.sessions)
            .get_mut(&session)
            .filter(|s| s.view.awaiting_me)
            .and_then(|s| s.decide.take())
            .ok_or_else(|| Error::Net("配对请求已结束".into()))?;
        let _ = decide.send(accept);
        Ok(())
    }

    /// 用户主动向 `address` 发起配对；`fingerprint` 为 mDNS 发现到的指纹，对不上时断开。
    pub fn pair_with(&self, address: String, fingerprint: Option<Fingerprint>) {
        let shared = Arc::clone(&self.shared);
        let intent = Intent {
            expect_name: None,
            expect_fp: fingerprint,
            initiated_by_user: true,
        };
        self.handle.spawn(async move {
            if let Err(e) = dial::dial_once(&shared, &address, intent).await {
                log::info!("向 {address} 发起配对失败：{e}");
                shared.notice(&format!("配对未完成：{e}"));
            }
        });
    }

    /// 取消配对：删除档案、断开连接，之后它再连进来也不弹配对请求（本次运行内）。
    pub fn unpair(&self, fingerprint: &Fingerprint) -> Result<()> {
        let removed = self.shared.store.remove(fingerprint)?;
        lock(&self.shared.rejected).insert(*fingerprint);
        let closing: Vec<Arc<Outbox>> = lock(&self.shared.conns)
            .values()
            .filter(|c| c.fingerprint == *fingerprint)
            .map(|c| Arc::clone(&c.outbox))
            .collect();
        for outbox in closing {
            outbox.close();
        }
        if let Some(peer) = removed {
            log::info!("已取消与 {} 的配对", peer.name);
            self.shared.emit(Event::PeerRemoved(peer.device_id()));
        }
        self.shared.changed();
        Ok(())
    }
}

impl Drop for TcpPeerLink {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

impl PeerLink for TcpPeerLink {
    fn send(&self, to: &DeviceId, msg: &Message) -> Result<()> {
        self.shared.outbox(to)?.push_control(Frame::message(msg)?);
        Ok(())
    }

    fn send_data(
        &self,
        to: &DeviceId,
        channel: ChannelId,
        handoff_id: HandoffId,
        payload: Payload,
    ) -> Result<()> {
        let outbox = self.shared.outbox(to)?;
        let id = self.shared.next_transfer.fetch_add(1, Ordering::Relaxed);
        outbox.push_transfer(OutgoingTransfer::new(channel, id, handoff_id, payload)?);
        Ok(())
    }

    fn set_local_state(&self, state: AgentState) {
        *lock(&self.shared.state) = state;
    }
}

struct Shared {
    me: PeerInfo,
    listen_port: u16,
    token: Option<Vec<u8>>,
    heartbeat: Duration,
    offline_after: Duration,
    identity: Arc<TlsIdentity>,
    store: Arc<PeerStore>,
    config_peers: Vec<PeerAddr>,
    observer: Option<Arc<dyn LinkObserver>>,
    events: mpsc::Sender<Event>,
    state: Mutex<AgentState>,
    conns: Mutex<HashMap<DeviceId, Conn>>,
    sessions: Mutex<HashMap<u64, Session>>,
    /// 本次运行中被用户拒绝或取消配对的指纹。
    rejected: Mutex<HashSet<Fingerprint>>,
    /// 正在拨号（含已经连上、连接仍在进行）的对端。
    dialing: Mutex<HashSet<DeviceId>>,
    next_generation: AtomicU64,
    next_transfer: AtomicU64,
    next_session: AtomicU64,
}

struct Conn {
    generation: u64,
    dialer: DeviceId,
    fingerprint: Fingerprint,
    outbox: Arc<Outbox>,
}

/// 进行中的配对会话。`decide` 被丢弃（会话被移除）时等待方视为取消。
struct Session {
    view: PairingSession,
    fingerprint: Fingerprint,
    dialer: DeviceId,
    decide: Option<oneshot::Sender<bool>>,
}

impl Shared {
    fn outbox(&self, peer: &DeviceId) -> Result<Arc<Outbox>> {
        lock(&self.conns)
            .get(peer)
            .map(|c| Arc::clone(&c.outbox))
            .ok_or_else(|| Error::Net(format!("{} 不在线", peer.0)))
    }

    fn is_connected(&self, peer: &DeviceId) -> bool {
        lock(&self.conns).contains_key(peer)
    }

    /// 两端同时互拨时各自保留由名称较小一方拨出的连接，结果一致。
    fn register(
        &self,
        peer: &DeviceId,
        dialer: DeviceId,
        fingerprint: Fingerprint,
        outbox: Arc<Outbox>,
    ) -> Option<u64> {
        let mut conns = lock(&self.conns);
        let preferred = self.preferred_dialer(peer);
        if let Some(existing) = conns.get(peer) {
            if dialer != preferred || existing.dialer == preferred {
                return None;
            }
            existing.outbox.close();
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        conns.insert(
            peer.clone(),
            Conn {
                generation,
                dialer,
                fingerprint,
                outbox,
            },
        );
        Some(generation)
    }

    /// 只有当前登记的连接结束才算对端离线；被替换的连接静默退出。
    fn unregister(&self, peer: &DeviceId, generation: u64) -> bool {
        let mut conns = lock(&self.conns);
        if conns.get(peer).is_some_and(|c| c.generation == generation) {
            conns.remove(peer);
            return true;
        }
        false
    }

    fn preferred_dialer(&self, peer: &DeviceId) -> DeviceId {
        self.me.device_id.clone().min(peer.clone())
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn changed(&self) {
        if let Some(observer) = &self.observer {
            observer.changed();
        }
    }

    fn notice(&self, message: &str) {
        if let Some(observer) = &self.observer {
            observer.notice(message);
        }
    }
}

/// 一条连接的发送队列；写任务从这里取帧。
struct Outbox {
    queue: Mutex<SendQueue>,
    wake: Notify,
    closed: watch::Sender<bool>,
}

impl Outbox {
    fn new() -> Self {
        Self {
            queue: Mutex::new(SendQueue::default()),
            wake: Notify::new(),
            closed: watch::channel(false).0,
        }
    }

    fn push_control(&self, frame: Frame) {
        lock(&self.queue).push_control(frame);
        self.wake.notify_one();
    }

    fn push_transfer(&self, transfer: OutgoingTransfer) {
        lock(&self.queue).push_transfer(transfer);
        self.wake.notify_one();
    }

    fn pop(&self) -> Option<Frame> {
        lock(&self.queue).next_frame()
    }

    fn close(&self) {
        self.closed.send_replace(true);
        self.wake.notify_one();
    }

    fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: TcpListener) {
    let acceptor = TlsAcceptor::from(Arc::clone(&shared.identity.server));
    loop {
        let (tcp, addr) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                log::warn!("接受连接失败：{e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
                continue;
            }
        };
        let shared = Arc::clone(&shared);
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let _ = tcp.set_nodelay(true);
            let result = async {
                let tls = timeout(HANDSHAKE_TIMEOUT, acceptor.accept(tcp))
                    .await
                    .map_err(|_| Error::Net("TLS 握手超时".into()))?
                    .map_err(io)?;
                let peer_fp = peer_fingerprint(tls.get_ref().1.peer_certificates())?;
                serve(shared, tls, Role::Server, peer_fp, addr, Intent::default()).await
            }
            .await;
            if let Err(e) = result {
                log::info!("来自 {addr} 的连接结束：{e}");
            }
        });
    }
}

/// 握手、登记，然后收发直到连接结束。握手失败返回错误；连接建立后的断开返回 `Ok`。
async fn serve<S>(
    shared: Arc<Shared>,
    stream: S,
    role: Role,
    peer_fp: Fingerprint,
    peer_addr: SocketAddr,
    intent: Intent,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut buf = Vec::new();
    let peer = handshake::run(
        &shared,
        &mut reader,
        &mut writer,
        &mut buf,
        role,
        peer_fp,
        peer_addr,
        intent,
    )
    .await?;

    let dialer = match role {
        Role::Client => shared.me.device_id.clone(),
        Role::Server => peer.device_id.clone(),
    };
    let outbox = Arc::new(Outbox::new());
    let Some(generation) = shared.register(&peer.device_id, dialer, peer_fp, Arc::clone(&outbox))
    else {
        return Ok(());
    };
    let peer_id = peer.device_id.clone();
    log::info!("已连接 {}", peer_id.0);
    shared.emit(Event::PeerOnline(peer));
    shared.changed();

    let writer_task = tokio::spawn(write_loop(Arc::clone(&shared), writer, Arc::clone(&outbox)));
    if let Err(e) = read_loop(&shared, &mut reader, &mut buf, &peer_id, &outbox).await {
        log::info!("与 {} 的连接中断：{e}", peer_id.0);
    }
    outbox.close();
    let _ = writer_task.await;
    if shared.unregister(&peer_id, generation) {
        log::info!("与 {} 的连接断开", peer_id.0);
        shared.emit(Event::PeerOffline(peer_id));
        shared.changed();
    }
    Ok(())
}

async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> Result<Frame> {
    loop {
        if let Some((frame, used)) = frame::decode(buf)? {
            buf.drain(..used);
            return Ok(frame);
        }
        if reader.read_buf(buf).await.map_err(io)? == 0 {
            return Err(Error::Net("连接已关闭".into()));
        }
    }
}

async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, message: &Message) -> Result<()> {
    writer
        .write_all(&Frame::message(message)?.encode()?)
        .await
        .map_err(io)?;
    writer.flush().await.map_err(io)
}

async fn read_loop<R: AsyncRead + Unpin>(
    shared: &Shared,
    reader: &mut R,
    buf: &mut Vec<u8>,
    peer: &DeviceId,
    outbox: &Outbox,
) -> Result<()> {
    let mut reassembler = Reassembler::default();
    let closed = outbox.closed.subscribe();
    tokio::pin! {
        let closed = wait_closed(closed);
    }
    loop {
        let frame = tokio::select! {
            read = timeout(shared.offline_after, read_frame(reader, buf)) => match read {
                Ok(frame) => frame?,
                Err(_) => return Err(Error::Net("心跳超时".into())),
            },
            _ = &mut closed => return Ok(()),
        };
        dispatch(shared, &mut reassembler, peer, frame);
    }
}

fn dispatch(shared: &Shared, reassembler: &mut Reassembler, peer: &DeviceId, frame: Frame) {
    if frame.channel == ChannelId::CONTROL {
        match frame.to_message() {
            Ok(
                Message::Heartbeat { .. }
                | Message::Hello { .. }
                | Message::PairCommit { .. }
                | Message::PairNonce { .. }
                | Message::PairReveal { .. }
                | Message::PairPending
                | Message::PairAccept,
            ) => {}
            Ok(message) => shared.emit(Event::Message {
                from: peer.clone(),
                message,
            }),
            Err(e) => log::warn!("{} 发来无法解析的控制帧：{e}", peer.0),
        }
        return;
    }
    match reassembler.accept(frame) {
        Ok(Some(done)) => shared.emit(Event::Data {
            from: peer.clone(),
            channel: done.channel,
            handoff_id: done.handoff_id,
            payload: done.payload,
        }),
        Ok(None) => {}
        Err(e) => log::warn!("{} 的数据传输出错：{e}", peer.0),
    }
}

async fn write_loop<W: AsyncWrite + Unpin>(
    shared: Arc<Shared>,
    mut writer: W,
    outbox: Arc<Outbox>,
) {
    let mut ticker = tokio::time::interval(shared.heartbeat);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        let mut wrote = false;
        while let Some(frame) = outbox.pop() {
            let bytes = match frame.encode() {
                Ok(bytes) => bytes,
                Err(e) => {
                    log::warn!("丢弃无法编码的帧：{e}");
                    continue;
                }
            };
            if let Err(e) = writer.write_all(&bytes).await {
                log::info!("发送失败：{e}");
                outbox.close();
                return;
            }
            wrote = true;
        }
        if wrote && writer.flush().await.is_err() {
            outbox.close();
            return;
        }
        if outbox.is_closed() {
            let _ = writer.shutdown().await;
            return;
        }
        tokio::select! {
            _ = ticker.tick() => {
                let state = *lock(&shared.state);
                if let Ok(frame) = Frame::message(&Message::Heartbeat { state }) {
                    outbox.push_control(frame);
                }
            }
            _ = outbox.wake.notified() => {}
        }
    }
}

async fn wait_closed(mut closed: watch::Receiver<bool>) {
    loop {
        if *closed.borrow_and_update() {
            return;
        }
        if closed.changed().await.is_err() {
            return;
        }
    }
}

fn peer_fingerprint(certs: Option<&[CertificateDer<'_>]>) -> Result<Fingerprint> {
    certs
        .and_then(|c| c.first())
        .map(auth::fingerprint)
        .ok_or_else(|| Error::Net("对端没有出示证书".into()))
}

fn io(e: std::io::Error) -> Error {
    Error::Net(e.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests;
