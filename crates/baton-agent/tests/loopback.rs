//! 两个已配对的 agent 在本机回环地址上经真实 TLS 链路完成双向交棒；显示器、电源为假实现。

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use baton_clipboard::{ClipboardBridge, EchoSet};
use baton_core::channel::ChannelId;
use baton_core::config::{Config, PeerConfig};
use baton_core::coordinator::{Coordinator, Ports, Timings};
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::{DeviceId, DisplayId, HandoffId, InputCode};
use baton_core::outbox::PeerRef;
use baton_core::participant::{HandoffParticipant, Payload};
use baton_core::ports::{
    AwakeGuard, ClipboardAccess, ClipboardContent, ClipboardRead, DisplayControl, DisplayInfo,
    Notifier, PeerLink, PowerControl,
};
use baton_core::protocol::Message;
use baton_core::state::AgentState;
use baton_core::wake::WakeChain;
use baton_core::{Error, Result};
use baton_input::MonitorKvmRouter;
use baton_net::{LinkConfig, PairedPeer, PeerAddr, PeerStore, TcpPeerLink, TlsIdentity};

const MAC: InputCode = InputCode(0x1B);
const WIN: InputCode = InputCode(0x0F);

/// 只响应当前活跃输入口的显示器。
type Monitor = Arc<Mutex<InputCode>>;

struct FakeDisplay {
    monitor: Monitor,
    mine: InputCode,
}

impl DisplayControl for FakeDisplay {
    fn list(&self) -> Result<Vec<DisplayInfo>> {
        Ok(vec![DisplayInfo {
            id: DisplayId("p275mv".into()),
            vendor: "TA".into(),
            model: "P275MV".into(),
            serial: None,
        }])
    }
    fn get_input(&self, _: &DisplayId) -> Result<InputCode> {
        let current = *self.monitor.lock().unwrap();
        (current == self.mine)
            .then_some(current)
            .ok_or_else(|| Error::Ddc("inactive input".into()))
    }
    fn set_input(&self, _: &DisplayId, input: InputCode) -> Result<()> {
        let mut current = self.monitor.lock().unwrap();
        if *current != self.mine {
            return Err(Error::Ddc("inactive input".into()));
        }
        *current = input;
        Ok(())
    }
}

struct FakePower(Arc<AtomicUsize>);

impl PowerControl for FakePower {
    fn stay_awake(&self, _: Duration) -> Result<AwakeGuard> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let guards = Arc::clone(&self.0);
        Ok(AwakeGuard::new(move || {
            guards.fetch_sub(1, Ordering::SeqCst);
        }))
    }
    fn wake_display(&self) -> Result<()> {
        Ok(())
    }
    fn restore_sleep(&self) -> Result<()> {
        Ok(())
    }
}

/// 经通道 1 写入的剪贴板；交棒不再打包。
struct Clipboard {
    received: Arc<Mutex<Vec<String>>>,
}

impl HandoffParticipant for Clipboard {
    fn channel(&self) -> ChannelId {
        ChannelId::CLIPBOARD
    }
    fn pack(&self, _: &DeviceId) -> Result<Option<Payload>> {
        Ok(None)
    }
    fn unpack(&self, _: &DeviceId, payload: Payload) -> Result<()> {
        let text = String::from_utf8(payload.data).map_err(|e| Error::Net(e.to_string()))?;
        self.received.lock().unwrap().push(text);
        Ok(())
    }
}

struct Quiet;

impl Notifier for Quiet {
    fn notify(&self, message: &str) {
        eprintln!("[notify] {message}");
    }
}

/// 转发给真实链路，同时记录协调器的状态变化。
struct ObservedLink {
    inner: Arc<TcpPeerLink>,
    state: Arc<Mutex<AgentState>>,
}

impl PeerLink for ObservedLink {
    fn send(&self, to: &DeviceId, msg: &Message) -> Result<()> {
        self.inner.send(to, msg)
    }
    fn send_data(&self, to: &DeviceId, c: ChannelId, id: HandoffId, p: Payload) -> Result<()> {
        self.inner.send_data(to, c, id, p)
    }
    fn set_local_state(&self, state: AgentState) {
        *self.state.lock().unwrap() = state;
        self.inner.set_local_state(state);
    }
}

struct Agent {
    events: Sender<Event>,
    thread: Option<JoinHandle<()>>,
    link: Arc<TcpPeerLink>,
    state: Arc<Mutex<AgentState>>,
    guards: Arc<AtomicUsize>,
    received: Arc<Mutex<Vec<String>>>,
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    fn start(
        name: &str,
        code: InputCode,
        identity: Arc<TlsIdentity>,
        store: Arc<PeerStore>,
        peer: &str,
        listen: SocketAddr,
        dial: Option<SocketAddr>,
        monitor: &Monitor,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = DeviceId(name.into());
        let mut link_config = LinkConfig::new(
            PeerInfo {
                device_id: me.clone(),
                platform: None,
                input_code: Some(code),
                wol_mac: None,
                ip: None,
            },
            listen,
            identity,
            store,
        );
        link_config.heartbeat_interval = Duration::from_millis(200);
        link_config.missed_heartbeats = 5;
        link_config.peers = dial
            .map(|addr| PeerAddr {
                device_id: DeviceId(peer.into()),
                address: addr.to_string(),
            })
            .into_iter()
            .collect();
        let link = Arc::new(TcpPeerLink::start(link_config, tx.clone()).unwrap());

        let mut config = Config::default();
        config.display.input_code = Some(code);
        config.handoff_timeout_ms = 3_000;
        config.peers.push(PeerConfig {
            name: peer.into(),
            address: None,
            wol_mac: None,
        });
        let state = Arc::new(Mutex::new(AgentState::Idle));
        let guards = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let ports = Ports {
            display: Box::new(FakeDisplay {
                monitor: Arc::clone(monitor),
                mine: code,
            }),
            power: Box::new(FakePower(Arc::clone(&guards))),
            input: Box::new(MonitorKvmRouter),
            wake: WakeChain::from_config(&[], Vec::new()),
            link: Arc::new(ObservedLink {
                inner: Arc::clone(&link),
                state: Arc::clone(&state),
            }),
            participants: vec![Arc::new(Clipboard {
                received: Arc::clone(&received),
            })],
            notifier: Box::new(Quiet),
        };
        let coordinator = Coordinator::new(me, config, ports).with_timings(Timings {
            wake_wait: Duration::from_secs(2),
            ready_timeout: Duration::from_secs(2),
            poll_interval: Duration::from_millis(50),
            receiver_awake: Duration::from_secs(10),
        });
        let thread = thread::spawn(move || coordinator.run(rx));
        Self {
            events: tx,
            thread: Some(thread),
            link,
            state,
            guards,
            received,
        }
    }

    fn state(&self) -> AgentState {
        *self.state.lock().unwrap()
    }

    fn shutdown(&mut self) {
        let _ = self.events.send(Event::Shutdown);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn free_addr() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn identity() -> Arc<TlsIdentity> {
    Arc::new(TlsIdentity::generate().unwrap())
}

/// 已与 `peer` 配对的档案。
fn pinned(peer: &str, identity: &TlsIdentity) -> Arc<PeerStore> {
    let store = PeerStore::in_memory();
    store
        .upsert(PairedPeer::new(peer, &identity.fingerprint))
        .unwrap();
    Arc::new(store)
}

fn wait_until(what: &str, cond: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cond() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("等待「{what}」超时");
}

#[test]
fn two_agents_hand_off_both_ways_over_tls() {
    let monitor: Monitor = Arc::new(Mutex::new(MAC));
    let win_addr = free_addr();
    let (mac_id, win_id) = (identity(), identity());
    let mac_store = pinned("win", &win_id);
    let win_store = pinned("mac", &mac_id);
    let mut win = Agent::start(
        "win", WIN, win_id, win_store, "mac", win_addr, None, &monitor,
    );
    let mut mac = Agent::start(
        "mac",
        MAC,
        mac_id,
        mac_store,
        "win",
        free_addr(),
        Some(win_addr),
        &monitor,
    );

    wait_until("初始状态", || {
        mac.state() == AgentState::Holding && win.state() == AgentState::Idle
    });
    let heartbeat = Message::Heartbeat {
        state: AgentState::Idle,
    };
    wait_until("建立连接", || {
        mac.link.send(&DeviceId("win".into()), &heartbeat).is_ok()
    });
    thread::sleep(Duration::from_millis(200));

    mac.link
        .send_data(
            &DeviceId("win".into()),
            ChannelId::CLIPBOARD,
            HandoffId::REALTIME,
            Payload {
                format: "text/plain".into(),
                data: "来自 mac".into(),
            },
        )
        .unwrap();
    wait_until("剪贴板到达 win", || {
        win.received.lock().unwrap().as_slice() == ["来自 mac"]
    });

    mac.events.send(Event::Hotkey).unwrap();
    wait_until("交棒给 win", || {
        *monitor.lock().unwrap() == WIN
            && win.state() == AgentState::Holding
            && mac.state() == AgentState::Idle
    });
    assert_eq!(
        mac.guards.load(Ordering::SeqCst),
        1,
        "交出后对端在线，保持唤醒"
    );
    assert_eq!(
        win.guards.load(Ordering::SeqCst),
        1,
        "接棒后对端仍在线，长时 guard 不释放"
    );

    win.events.send(Event::Hotkey).unwrap();
    wait_until("交棒回 mac", || {
        *monitor.lock().unwrap() == MAC
            && mac.state() == AgentState::Holding
            && win.state() == AgentState::Idle
    });
    win.link
        .send_data(
            &DeviceId("mac".into()),
            ChannelId::CLIPBOARD,
            HandoffId::REALTIME,
            Payload {
                format: "text/plain".into(),
                data: "来自 win".into(),
            },
        )
        .unwrap();
    wait_until("剪贴板到达 mac", || {
        mac.received.lock().unwrap().as_slice() == ["来自 win"]
    });
    assert_eq!(win.guards.load(Ordering::SeqCst), 1);
    assert_eq!(mac.guards.load(Ordering::SeqCst), 1, "重新持棒时对端仍在线");

    mac.shutdown();
    win.shutdown();
    assert_eq!(win.guards.load(Ordering::SeqCst), 0, "退出时释放保持唤醒");
}

struct FakeClip(Mutex<ClipboardRead>);

impl ClipboardAccess for FakeClip {
    fn read(&self) -> Result<ClipboardRead> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn write(&self, _: &ClipboardContent) -> Result<()> {
        Ok(())
    }
}

#[test]
fn clipboard_outbox_flushes_after_fake_offline() {
    let monitor: Monitor = Arc::new(Mutex::new(MAC));
    let win_addr = free_addr();
    let (mac_id, win_id) = (identity(), identity());
    let mac_store = pinned("win", &win_id);
    let win_store = pinned("mac", &mac_id);
    let mut win = Agent::start(
        "win", WIN, win_id, win_store, "mac", win_addr, None, &monitor,
    );
    let mut mac = Agent::start(
        "mac",
        MAC,
        mac_id,
        mac_store,
        "win",
        free_addr(),
        Some(win_addr),
        &monitor,
    );
    let heartbeat = Message::Heartbeat {
        state: AgentState::Idle,
    };
    wait_until("建立连接", || {
        mac.link.send(&DeviceId("win".into()), &heartbeat).is_ok()
    });

    let access = Arc::new(FakeClip(Mutex::new(ClipboardRead::Content(
        ClipboardContent::Text("queued".into()),
    ))));
    let bridge = ClipboardBridge::new(
        access,
        Arc::new(AtomicBool::new(true)),
        Arc::new(Quiet),
        Arc::new(Mutex::new(EchoSet::new())),
        10,
    );
    let mut peer = PeerRef {
        key: "fp-win".into(),
        to: DeviceId("win".into()),
        online: false,
    };
    bridge.tick(&[peer.clone()], |_, _| unreachable!());
    assert_eq!(bridge.pending(), 1);

    peer.online = true;
    let link = Arc::clone(&mac.link);
    bridge.tick(&[peer], |to, payload| {
        link.send_data(to, ChannelId::CLIPBOARD, HandoffId::REALTIME, payload)
    });
    wait_until("冲刷到达 win", || {
        win.received.lock().unwrap().as_slice() == ["queued"]
    });
    assert_eq!(bridge.pending(), 0);

    mac.shutdown();
    win.shutdown();
}
