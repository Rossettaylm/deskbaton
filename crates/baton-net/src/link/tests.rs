use std::net::TcpListener as StdListener;
use std::time::Instant;

use baton_core::ids::{InputCode, Platform};
use baton_core::protocol::PROTOCOL_VERSION;

use super::*;
use crate::discovery::Discovered;

const WAIT: Duration = Duration::from_secs(5);

fn info(name: &str, code: u16) -> PeerInfo {
    PeerInfo {
        device_id: DeviceId(name.into()),
        platform: Some(if name == "win" {
            Platform::Windows
        } else {
            Platform::Macos
        }),
        input_code: Some(InputCode(code)),
        wol_mac: None,
        ip: None,
    }
}

fn free_addr() -> SocketAddr {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

/// 一台测试设备的持久部分：身份与档案在重启之间保留。
struct Device {
    name: &'static str,
    identity: Arc<TlsIdentity>,
    store: Arc<PeerStore>,
}

impl Device {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            identity: Arc::new(TlsIdentity::generate().unwrap()),
            store: Arc::new(PeerStore::in_memory()),
        }
    }

    fn config(&self, listen: SocketAddr, peers: &[(&str, SocketAddr)]) -> LinkConfig {
        let mut config = LinkConfig::new(
            info(self.name, 0x1B),
            listen,
            Arc::clone(&self.identity),
            Arc::clone(&self.store),
        );
        config.heartbeat_interval = Duration::from_millis(100);
        config.missed_heartbeats = 5;
        config.peers = peers
            .iter()
            .map(|(id, addr)| PeerAddr {
                device_id: DeviceId((*id).into()),
                address: addr.to_string(),
            })
            .collect();
        config
    }
}

fn start(config: LinkConfig) -> (TcpPeerLink, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();
    (TcpPeerLink::start(config, tx).unwrap(), rx)
}

fn with_token(mut config: LinkConfig, token: &str) -> LinkConfig {
    config.token = Some(token.into());
    config
}

/// 把配对界面收到的通知转成可等待的消息。
struct Probe(Mutex<mpsc::Sender<String>>);

impl LinkObserver for Probe {
    fn changed(&self) {
        let _ = lock(&self.0).send(String::new());
    }
    fn notice(&self, message: &str) {
        let _ = lock(&self.0).send(message.to_owned());
    }
}

fn with_probe(mut config: LinkConfig) -> (LinkConfig, mpsc::Receiver<String>) {
    let (tx, rx) = mpsc::channel();
    config.observer = Some(Arc::new(Probe(Mutex::new(tx))));
    (config, rx)
}

fn expect(rx: &mpsc::Receiver<Event>, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(event) if pred(&event) => return event,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("等待 {what} 超时");
}

fn none_within(rx: &mpsc::Receiver<Event>, window: Duration, pred: impl Fn(&Event) -> bool) {
    let deadline = Instant::now() + window;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Ok(event) = rx.recv_timeout(left) {
            assert!(!pred(&event), "不应出现的事件：{event:?}");
        }
    }
}

fn wait_session(link: &TcpPeerLink, notes: &mpsc::Receiver<String>) -> PairingSession {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(s) = link.snapshot().sessions.into_iter().find(|s| s.awaiting_me) {
            return s;
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("等待配对请求超时");
        let _ = notes.recv_timeout(left.min(Duration::from_millis(50)));
    }
}

fn wait_notice(notes: &mpsc::Receiver<String>, needle: &str) -> String {
    let deadline = Instant::now() + WAIT;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match notes.recv_timeout(left) {
            Ok(note) if note.contains(needle) => return note,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    panic!("等待提示「{needle}」超时");
}

fn is_online(name: &str) -> impl Fn(&Event) -> bool + '_ {
    move |e| matches!(e, Event::PeerOnline(p) if p.device_id.0 == name)
}

fn is_offline(name: &str) -> impl Fn(&Event) -> bool + '_ {
    move |e| matches!(e, Event::PeerOffline(p) if p.0 == name)
}

#[test]
fn handshakes_and_exchanges_messages_and_data() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, b_rx) = start(with_token(win_dev.config(local(), &[]), "token"));
    let (a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    ));

    let online = expect(&a_rx, "win 上线", is_online("win"));
    assert_eq!(online, Event::PeerOnline(info("win", 0x1B)));
    expect(&b_rx, "mac 上线", is_online("mac"));

    let win = DeviceId("win".into());
    let mac = DeviceId("mac".into());
    a.send(
        &win,
        &Message::Prepare {
            handoff_id: HandoffId(1),
        },
    )
    .unwrap();
    expect(&b_rx, "prepare", |e| {
        *e == Event::Message {
            from: mac.clone(),
            message: Message::Prepare {
                handoff_id: HandoffId(1),
            },
        }
    });
    b.send(
        &mac,
        &Message::Ready {
            handoff_id: HandoffId(1),
        },
    )
    .unwrap();
    expect(&a_rx, "ready", |e| {
        matches!(
            e,
            Event::Message {
                message: Message::Ready { .. },
                ..
            }
        )
    });

    let payload = Payload {
        format: "image/png".into(),
        data: (0..200_000).map(|i| (i % 253) as u8).collect(),
    };
    a.send_data(&win, ChannelId::CLIPBOARD, HandoffId(1), payload.clone())
        .unwrap();
    let data = expect(&b_rx, "数据", |e| matches!(e, Event::Data { .. }));
    assert_eq!(
        data,
        Event::Data {
            from: mac,
            channel: ChannelId::CLIPBOARD,
            handoff_id: HandoffId(1),
            payload,
        }
    );
}

#[test]
fn token_pairs_once_and_later_connects_without_it() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let win_addr = free_addr();
    {
        let (_b, b_rx) = start(with_token(win_dev.config(win_addr, &[]), "token"));
        let (_a, a_rx) = start(with_token(
            mac_dev.config(local(), &[("win", win_addr)]),
            "token",
        ));
        expect(&a_rx, "win 上线", is_online("win"));
        expect(&b_rx, "mac 上线", is_online("mac"));
    }
    assert!(mac_dev.store.find(&win_dev.identity.fingerprint).is_some());
    let pinned = win_dev.store.find(&mac_dev.identity.fingerprint).unwrap();
    assert_eq!(pinned.name, "mac");
    assert_eq!(pinned.input_code, Some(InputCode(0x1B)));
    assert_eq!(pinned.platform, Some(Platform::Macos));

    let (_b, b_rx) = start(win_dev.config(win_addr, &[]));
    let (a, a_rx) = start(mac_dev.config(local(), &[("win", win_addr)]));
    expect(&a_rx, "win 无令牌上线", is_online("win"));
    expect(&b_rx, "mac 无令牌上线", is_online("mac"));
    let snapshot = a.snapshot();
    assert_eq!(snapshot.paired.len(), 1);
    assert!(snapshot.paired[0].online);
}

#[test]
fn learned_input_code_reaches_peer_and_its_profile() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let mut win_config = with_token(win_dev.config(local(), &[]), "token");
    win_config.me.input_code = None;
    let (b, b_rx) = start(win_config);
    let (_a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    ));
    let online = expect(&a_rx, "win 上线", is_online("win"));
    assert!(matches!(online, Event::PeerOnline(p) if p.input_code.is_none()));
    expect(&b_rx, "mac 上线", is_online("mac"));

    b.set_local_input_code(Some(InputCode(0x10)));
    let updated = expect(&a_rx, "win 的编码", |e| {
        matches!(e, Event::PeerUpdated(_))
    });
    let Event::PeerUpdated(peer) = updated else {
        unreachable!()
    };
    assert_eq!(peer.device_id.0, "win");
    assert_eq!(peer.input_code, Some(InputCode(0x10)));
    let profile = mac_dev.store.find(&win_dev.identity.fingerprint).unwrap();
    assert_eq!(profile.input_code, Some(InputCode(0x10)));

    b.set_local_input_code(None);
    expect(
        &a_rx,
        "win 清空编码",
        |e| matches!(e, Event::PeerUpdated(p) if p.input_code.is_none()),
    );
    let profile = mac_dev.store.find(&win_dev.identity.fingerprint).unwrap();
    assert_eq!(profile.input_code, None);
}

fn wait_for(what: &str, within: Duration, pred: impl Fn() -> bool) {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if pred() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("等待 {what} 超时");
}

#[test]
fn peer_state_change_reaches_snapshot_without_waiting_for_heartbeat() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let slow = |mut config: LinkConfig| {
        config.heartbeat_interval = Duration::from_secs(10);
        config
    };
    let (b, _b_rx) = start(slow(with_token(win_dev.config(local(), &[]), "token")));
    let (a, a_rx) = start(slow(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    )));
    expect(&a_rx, "win 上线", is_online("win"));
    let win_holding = || a.snapshot().paired.first().is_some_and(|s| s.holding);
    assert!(!win_holding());

    b.set_local_state(AgentState::Holding);
    wait_for("win 持棒", Duration::from_secs(1), win_holding);
    b.set_local_state(AgentState::Idle);
    wait_for("win 交出", Duration::from_secs(1), || !win_holding());
}

#[test]
fn rejects_wrong_token() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, b_rx) = start(with_token(win_dev.config(local(), &[]), "right"));
    let (_a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "wrong",
    ));
    none_within(&a_rx, Duration::from_millis(800), is_online("win"));
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
    assert!(win_dev.store.list().is_empty());
}

#[test]
fn unpaired_peer_is_rejected_without_ui() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, b_rx) = start(win_dev.config(local(), &[]));
    let (a, a_rx) = start(mac_dev.config(local(), &[]));
    a.pair_with(b.local_addr().to_string(), None);
    none_within(&a_rx, Duration::from_millis(800), is_online("win"));
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
}

#[test]
fn interactive_pairing_needs_both_ends_to_agree() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (win_config, win_notes) = with_probe(win_dev.config(local(), &[]));
    let (mac_config, mac_notes) = with_probe(mac_dev.config(local(), &[]));
    let (b, b_rx) = start(win_config);
    let (a, a_rx) = start(mac_config);

    a.pair_with(
        b.local_addr().to_string(),
        Some(win_dev.identity.fingerprint),
    );
    let on_mac = wait_session(&a, &mac_notes);
    let on_win = wait_session(&b, &win_notes);
    assert_eq!(on_mac.peer.0, "win");
    assert_eq!(on_win.peer.0, "mac");
    assert_eq!(on_mac.platform, Some(Platform::Windows));
    assert_eq!(on_win.platform, Some(Platform::Macos));

    a.confirm_pairing(on_mac.id, true).unwrap();
    none_within(&a_rx, Duration::from_millis(300), is_online("win"));
    b.confirm_pairing(on_win.id, true).unwrap();
    expect(&a_rx, "win 上线", is_online("win"));
    expect(&b_rx, "mac 上线", is_online("mac"));
    wait_notice(&mac_notes, "已与 win 配对");
    assert!(a.snapshot().sessions.is_empty());
    assert!(mac_dev.store.find(&win_dev.identity.fingerprint).is_some());
    assert!(win_dev.store.find(&mac_dev.identity.fingerprint).is_some());
}

#[test]
fn rejected_pairing_leaves_both_stores_untouched() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (win_config, win_notes) = with_probe(win_dev.config(local(), &[]));
    let (mac_config, mac_notes) = with_probe(mac_dev.config(local(), &[]));
    let (b, b_rx) = start(win_config);
    let (a, a_rx) = start(mac_config);

    a.pair_with(b.local_addr().to_string(), None);
    let on_mac = wait_session(&a, &mac_notes);
    let on_win = wait_session(&b, &win_notes);
    a.confirm_pairing(on_mac.id, true).unwrap();
    b.confirm_pairing(on_win.id, false).unwrap();
    wait_notice(&mac_notes, "未完成");
    none_within(&a_rx, Duration::from_millis(300), is_online("win"));
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
    assert!(mac_dev.store.list().is_empty());
    assert!(win_dev.store.list().is_empty());

    // 被拒绝过的设备再连进来时不再弹出配对请求。win 直接断开，mac 的会话可能一闪而过，只等结果提示。
    a.pair_with(b.local_addr().to_string(), None);
    wait_notice(&mac_notes, "未完成");
    assert!(b.snapshot().sessions.is_empty());
    assert!(win_dev.store.list().is_empty());
}

#[test]
fn pinned_name_with_another_certificate_is_not_trusted() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    mac_dev
        .store
        .upsert(PairedPeer::new("win", &[7; 32]))
        .unwrap();
    let (b, b_rx) = start(win_dev.config(local(), &[]));
    let (_a, a_rx) = start(mac_dev.config(local(), &[("win", b.local_addr())]));
    none_within(&a_rx, Duration::from_millis(800), is_online("win"));
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
}

#[test]
fn unpair_disconnects_and_forgets() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, _b_rx) = start(with_token(win_dev.config(local(), &[]), "token"));
    let (a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    ));
    expect(&a_rx, "win 上线", is_online("win"));
    a.unpair(&win_dev.identity.fingerprint).unwrap();
    // 断开与移除分别在网络线程和调用方线程上报告，先后不定。
    let (mut removed, mut offline) = (false, false);
    while !(removed && offline) {
        let event = expect(&a_rx, "win 被移除并离线", |e| {
            matches!(e, Event::PeerRemoved(p) if p.0 == "win") || is_offline("win")(e)
        });
        removed |= matches!(event, Event::PeerRemoved(_));
        offline |= matches!(event, Event::PeerOffline(_));
    }
    assert!(mac_dev.store.list().is_empty());
}

#[test]
fn rejects_peer_with_unexpected_name() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, b_rx) = start(with_token(win_dev.config(local(), &[]), "token"));
    let (_a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("desktop", b.local_addr())]),
        "token",
    ));
    none_within(&a_rx, Duration::from_millis(800), |e| {
        matches!(e, Event::PeerOnline(_))
    });
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
}

#[test]
fn reports_offline_when_peer_goes_away() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, _b_rx) = start(with_token(win_dev.config(local(), &[]), "token"));
    let (_a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    ));
    expect(&a_rx, "win 上线", is_online("win"));
    drop(b);
    expect(&a_rx, "win 离线", is_offline("win"));
}

#[test]
fn heartbeats_keep_idle_connection_alive() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (b, _b_rx) = start(with_token(win_dev.config(local(), &[]), "token"));
    let (_a, a_rx) = start(with_token(
        mac_dev.config(local(), &[("win", b.local_addr())]),
        "token",
    ));
    expect(&a_rx, "win 上线", is_online("win"));
    none_within(&a_rx, Duration::from_millis(1500), is_offline("win"));
}

#[test]
fn simultaneous_dials_settle_on_one_connection() {
    let (mac_dev, win_dev) = (Device::new("mac"), Device::new("win"));
    let (a_addr, b_addr) = (free_addr(), free_addr());
    let (a, a_rx) = start(with_token(
        mac_dev.config(a_addr, &[("win", b_addr)]),
        "token",
    ));
    let (b, b_rx) = start(with_token(
        win_dev.config(b_addr, &[("mac", a_addr)]),
        "token",
    ));
    expect(&a_rx, "win 上线", is_online("win"));
    expect(&b_rx, "mac 上线", is_online("mac"));
    none_within(&a_rx, Duration::from_millis(2500), is_offline("win"));

    b.send(
        &DeviceId("mac".into()),
        &Message::Taken {
            handoff_id: HandoffId(3),
        },
    )
    .unwrap();
    expect(&a_rx, "taken", |e| {
        matches!(
            e,
            Event::Message {
                message: Message::Taken { .. },
                ..
            }
        )
    });
    drop(a);
}

#[test]
fn sending_to_offline_peer_fails() {
    let (a, _rx) = start(Device::new("mac").config(local(), &[]));
    assert!(
        a.send(
            &DeviceId("win".into()),
            &Message::Taken {
                handoff_id: HandoffId(1)
            }
        )
        .is_err()
    );
}

#[test]
fn empty_token_is_rejected() {
    let (tx, _rx) = mpsc::channel();
    let config = with_token(Device::new("mac").config(local(), &[]), "");
    assert!(matches!(
        TcpPeerLink::start(config, tx),
        Err(Error::Config(_))
    ));
}

#[test]
fn candidates_skip_paired_and_duplicate_devices() {
    let paired = [PairedPeer::new("win", &[1; 32])];
    let discovered = [
        Discovered {
            name: DeviceId("win".into()),
            fingerprint: auth::to_hex(&[1; 32]),
            addresses: vec!["192.168.1.20:47800".parse().unwrap()],
            version: Some(PROTOCOL_VERSION),
        },
        Discovered {
            name: DeviceId("mini".into()),
            fingerprint: auth::to_hex(&[2; 32]),
            addresses: vec!["192.168.1.30:47800".parse().unwrap()],
            version: Some(PROTOCOL_VERSION),
        },
    ];
    let configured = [
        PeerAddr {
            device_id: DeviceId("mini".into()),
            address: "mini.local:47800".into(),
        },
        PeerAddr {
            device_id: DeviceId("nas".into()),
            address: "192.168.1.40:47800".into(),
        },
    ];
    assert_eq!(
        candidates(&paired, &discovered, &configured),
        [
            Candidate {
                name: DeviceId("mini".into()),
                fingerprint: Some(auth::to_hex(&[2; 32])),
                address: "192.168.1.30:47800".into(),
            },
            Candidate {
                name: DeviceId("nas".into()),
                fingerprint: None,
                address: "192.168.1.40:47800".into(),
            },
        ]
    );
}

/// 需要本机网卡允许组播，CI 上不一定具备，手动运行：`cargo test -p baton-net -- --ignored`。
#[test]
#[ignore]
fn devices_find_each_other_over_mdns() {
    let (mac_dev, win_dev) = (Device::new("mdns-mac"), Device::new("mdns-win"));
    let mut mac_config = mac_dev.config("0.0.0.0:0".parse().unwrap(), &[]);
    let mut win_config = win_dev.config("0.0.0.0:0".parse().unwrap(), &[]);
    mac_config.discovery = true;
    win_config.discovery = true;
    let (a, _a_rx) = start(mac_config);
    let (_b, _b_rx) = start(win_config);
    let win_fp = auth::to_hex(&win_dev.identity.fingerprint);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if a.snapshot()
            .candidates
            .iter()
            .any(|c| c.fingerprint.as_deref() == Some(win_fp.as_str()))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("10 秒内没有经 mDNS 发现对方");
}

/// 档案里没有地址、配置里也没写，只靠 mDNS 解析到的地址连上。手动运行，同上。
#[test]
#[ignore]
fn paired_devices_connect_via_mdns_address() {
    let (mac_dev, win_dev) = (Device::new("mdns-mac2"), Device::new("mdns-win2"));
    mac_dev
        .store
        .upsert(PairedPeer::new("mdns-win2", &win_dev.identity.fingerprint))
        .unwrap();
    win_dev
        .store
        .upsert(PairedPeer::new("mdns-mac2", &mac_dev.identity.fingerprint))
        .unwrap();
    let mut mac_config = mac_dev.config("0.0.0.0:0".parse().unwrap(), &[]);
    let mut win_config = win_dev.config("0.0.0.0:0".parse().unwrap(), &[]);
    mac_config.discovery = true;
    win_config.discovery = true;
    let (_a, a_rx) = start(mac_config);
    let (_b, _b_rx) = start(win_config);
    let deadline = Instant::now() + Duration::from_secs(10);
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        if let Ok(Event::PeerOnline(p)) = a_rx.recv_timeout(left)
            && p.device_id.0 == "mdns-win2"
        {
            return;
        }
    }
    panic!("10 秒内没有经 mDNS 地址连上");
}
