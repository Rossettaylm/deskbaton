//! TCP + TLS 链路：每对 agent 一条连接，控制帧优先；握手、心跳与离线判定都在这里维护。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::time::Duration;

use baton_core::channel::ChannelId;
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::{DeviceId, HandoffId};
use baton_core::participant::Payload;
use baton_core::ports::PeerLink;
use baton_core::protocol::{Message, PROTOCOL_VERSION};
use baton_core::state::AgentState;
use baton_core::{Error, Result};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;
use tokio::sync::{Notify, watch};
use tokio::time::{MissedTickBehavior, timeout};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::auth::{self, Fingerprint, Role, TlsIdentity};
use crate::frame::{self, Frame};
use crate::mux::SendQueue;
use crate::transfer::{OutgoingTransfer, Reassembler};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REDIAL_INTERVAL: Duration = Duration::from_secs(2);

/// 主动拨号的对端：名称必须与对端配置的本机名称一致。
#[derive(Debug, Clone)]
pub struct PeerAddr {
    pub device_id: DeviceId,
    /// `host:port`。
    pub address: String,
}

#[derive(Debug, Clone)]
pub struct LinkConfig {
    pub me: PeerInfo,
    pub listen: SocketAddr,
    pub peers: Vec<PeerAddr>,
    pub token: String,
    pub heartbeat_interval: Duration,
    /// 连续这么多个心跳周期收不到任何帧即判定离线。
    pub missed_heartbeats: u32,
}

impl LinkConfig {
    pub fn new(me: PeerInfo, listen: SocketAddr, token: String) -> Self {
        Self {
            me,
            listen,
            peers: Vec::new(),
            token,
            heartbeat_interval: Duration::from_secs(2),
            missed_heartbeats: 3,
        }
    }
}

pub struct TcpPeerLink {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    runtime: Option<Runtime>,
}

impl TcpPeerLink {
    /// 开始监听并拨号配置的对端；收到的一切以 [`Event`] 发往 `events`。
    pub fn start(config: LinkConfig, events: mpsc::Sender<Event>) -> Result<Self> {
        if config.token.is_empty() {
            return Err(Error::Config("network.token 不能为空".into()));
        }
        let identity = TlsIdentity::generate()?;
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
            token: config.token.into_bytes(),
            heartbeat: interval,
            offline_after: interval * config.missed_heartbeats.max(1),
            identity,
            events,
            state: Mutex::new(AgentState::Idle),
            conns: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(0),
            next_transfer: AtomicU64::new(0),
        });
        runtime.spawn(accept_loop(Arc::clone(&shared), listener));
        for peer in config.peers {
            runtime.spawn(dial_loop(Arc::clone(&shared), peer));
        }
        Ok(Self {
            shared,
            local_addr,
            runtime: Some(runtime),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
    token: Vec<u8>,
    heartbeat: Duration,
    offline_after: Duration,
    identity: TlsIdentity,
    events: mpsc::Sender<Event>,
    state: Mutex<AgentState>,
    conns: Mutex<HashMap<DeviceId, Conn>>,
    next_generation: AtomicU64,
    next_transfer: AtomicU64,
}

struct Conn {
    generation: u64,
    dialer: DeviceId,
    outbox: Arc<Outbox>,
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
    fn register(&self, peer: &DeviceId, dialer: DeviceId, outbox: Arc<Outbox>) -> Option<u64> {
        let mut conns = lock(&self.conns);
        let preferred = self.me.device_id.clone().min(peer.clone());
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

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }

    fn hello(&self, role: Role, client_fp: &Fingerprint, server_fp: &Fingerprint) -> Result<Frame> {
        let proof = auth::proof(&self.token, role, &self.me.device_id, client_fp, server_fp)?;
        Frame::message(&Message::Hello {
            device_id: self.me.device_id.clone(),
            protocol_version: PROTOCOL_VERSION,
            input_code: self.me.input_code,
            wol_mac: self.me.wol_mac,
            auth: auth::to_hex(&proof),
        })
    }

    fn check_hello(
        &self,
        frame: Frame,
        peer_role: Role,
        client_fp: &Fingerprint,
        server_fp: &Fingerprint,
    ) -> Result<PeerInfo> {
        let Message::Hello {
            device_id,
            protocol_version,
            input_code,
            wol_mac,
            auth,
        } = frame.to_message()?
        else {
            return Err(Error::Net("对端没有先发送 hello".into()));
        };
        if protocol_version != PROTOCOL_VERSION {
            return Err(Error::Net(format!(
                "协议版本不一致：本机 {PROTOCOL_VERSION}，{} 为 {protocol_version}",
                device_id.0
            )));
        }
        let claimed = auth::from_hex(&auth).unwrap_or_default();
        if !auth::verify(
            &self.token,
            peer_role,
            &device_id,
            client_fp,
            server_fp,
            &claimed,
        ) {
            return Err(Error::Net(format!(
                "{} 认证失败，请检查两端 network.token 是否一致",
                device_id.0
            )));
        }
        if device_id == self.me.device_id {
            return Err(Error::Net("对端名称与本机相同".into()));
        }
        Ok(PeerInfo {
            device_id,
            input_code,
            wol_mac,
        })
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
                serve(shared, tls, Role::Server, peer_fp, None).await
            }
            .await;
            if let Err(e) = result {
                log::info!("来自 {addr} 的连接结束：{e}");
            }
        });
    }
}

async fn dial_loop(shared: Arc<Shared>, peer: PeerAddr) {
    let connector = TlsConnector::from(Arc::clone(&shared.identity.client));
    if shared.me.device_id > peer.device_id {
        // 名称较大的一方晚一点拨号，减少两端同时互拨。
        tokio::time::sleep(REDIAL_INTERVAL / 2).await;
    }
    loop {
        if !shared.is_connected(&peer.device_id)
            && let Err(e) = dial_once(&shared, &connector, &peer).await
        {
            log::debug!("连接 {}（{}）失败：{e}", peer.device_id.0, peer.address);
        }
        tokio::time::sleep(REDIAL_INTERVAL).await;
    }
}

async fn dial_once(shared: &Arc<Shared>, connector: &TlsConnector, peer: &PeerAddr) -> Result<()> {
    let tcp = timeout(CONNECT_TIMEOUT, TcpStream::connect(&peer.address))
        .await
        .map_err(|_| Error::Net("连接超时".into()))?
        .map_err(io)?;
    let _ = tcp.set_nodelay(true);
    let name = ServerName::try_from(auth::SERVER_NAME).map_err(|e| Error::Net(e.to_string()))?;
    let tls = timeout(HANDSHAKE_TIMEOUT, connector.connect(name, tcp))
        .await
        .map_err(|_| Error::Net("TLS 握手超时".into()))?
        .map_err(io)?;
    let peer_fp = peer_fingerprint(tls.get_ref().1.peer_certificates())?;
    serve(
        Arc::clone(shared),
        tls,
        Role::Client,
        peer_fp,
        Some(&peer.device_id),
    )
    .await
}

/// 握手、登记，然后收发直到连接结束。
async fn serve<S>(
    shared: Arc<Shared>,
    stream: S,
    role: Role,
    peer_fp: Fingerprint,
    expected: Option<&DeviceId>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let own_fp = shared.identity.fingerprint;
    let (client_fp, server_fp) = match role {
        Role::Client => (own_fp, peer_fp),
        Role::Server => (peer_fp, own_fp),
    };
    let (mut reader, mut writer) = tokio::io::split(stream);
    let hello = shared.hello(role, &client_fp, &server_fp)?.encode()?;
    writer.write_all(&hello).await.map_err(io)?;
    writer.flush().await.map_err(io)?;

    let mut buf = Vec::new();
    let first = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader, &mut buf))
        .await
        .map_err(|_| Error::Net("等待 hello 超时".into()))??;
    let peer = shared.check_hello(first, role.peer(), &client_fp, &server_fp)?;
    if let Some(expected) = expected.filter(|e| **e != peer.device_id) {
        return Err(Error::Net(format!(
            "对端名称为 {}，与配置的 {} 不符",
            peer.device_id.0, expected.0
        )));
    }
    // 拨号方还要校验对端名称；接受方等到拨号方发来第一帧（登记后立即发出的心跳）才算连接建立，
    // 否则拨号方拒绝后这里会误报一次上线和离线。
    let confirmation = match role {
        Role::Client => None,
        Role::Server => Some(
            timeout(HANDSHAKE_TIMEOUT, read_frame(&mut reader, &mut buf))
                .await
                .map_err(|_| Error::Net("等待对端确认超时".into()))??,
        ),
    };
    let dialer = match role {
        Role::Client => shared.me.device_id.clone(),
        Role::Server => peer.device_id.clone(),
    };
    let outbox = Arc::new(Outbox::new());
    let Some(generation) = shared.register(&peer.device_id, dialer, Arc::clone(&outbox)) else {
        return Ok(());
    };
    let peer_id = peer.device_id.clone();
    log::info!("已连接 {}", peer_id.0);
    shared.emit(Event::PeerOnline(peer));

    let writer_task = tokio::spawn(write_loop(Arc::clone(&shared), writer, Arc::clone(&outbox)));
    let result = read_loop(
        &shared,
        &mut reader,
        &mut buf,
        &peer_id,
        &outbox,
        confirmation,
    )
    .await;
    outbox.close();
    let _ = writer_task.await;
    if shared.unregister(&peer_id, generation) {
        log::info!("与 {} 的连接断开", peer_id.0);
        shared.emit(Event::PeerOffline(peer_id));
    }
    result
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

async fn read_loop<R: AsyncRead + Unpin>(
    shared: &Shared,
    reader: &mut R,
    buf: &mut Vec<u8>,
    peer: &DeviceId,
    outbox: &Outbox,
    first: Option<Frame>,
) -> Result<()> {
    let mut reassembler = Reassembler::default();
    if let Some(frame) = first {
        dispatch(shared, &mut reassembler, peer, frame);
    }
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
            Ok(Message::Heartbeat { .. } | Message::Hello { .. }) => {}
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
