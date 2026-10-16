use std::net::TcpListener as StdListener;
use std::time::Instant;

use baton_core::ids::InputCode;

use super::*;

const WAIT: Duration = Duration::from_secs(5);

fn info(name: &str, code: u16) -> PeerInfo {
    PeerInfo {
        device_id: DeviceId(name.into()),
        input_code: Some(InputCode(code)),
        wol_mac: None,
    }
}

fn free_addr() -> SocketAddr {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn start(
    name: &str,
    listen: SocketAddr,
    peers: &[(&str, SocketAddr)],
    token: &str,
) -> (TcpPeerLink, mpsc::Receiver<Event>) {
    let mut config = LinkConfig::new(info(name, 0x1B), listen, token.into());
    config.heartbeat_interval = Duration::from_millis(100);
    config.missed_heartbeats = 5;
    config.peers = peers
        .iter()
        .map(|(id, addr)| PeerAddr {
            device_id: DeviceId((*id).into()),
            address: addr.to_string(),
        })
        .collect();
    let (tx, rx) = mpsc::channel();
    (TcpPeerLink::start(config, tx).unwrap(), rx)
}

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
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

fn is_online(name: &str) -> impl Fn(&Event) -> bool + '_ {
    move |e| matches!(e, Event::PeerOnline(p) if p.device_id.0 == name)
}

fn is_offline(name: &str) -> impl Fn(&Event) -> bool + '_ {
    move |e| matches!(e, Event::PeerOffline(p) if p.0 == name)
}

#[test]
fn handshakes_and_exchanges_messages_and_data() {
    let (b, b_rx) = start("win", local(), &[], "token");
    let (a, a_rx) = start("mac", local(), &[("win", b.local_addr())], "token");

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
fn rejects_wrong_token() {
    let (b, b_rx) = start("win", local(), &[], "right");
    let (_a, a_rx) = start("mac", local(), &[("win", b.local_addr())], "wrong");
    none_within(&a_rx, Duration::from_millis(800), is_online("win"));
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
}

#[test]
fn rejects_peer_with_unexpected_name() {
    let (b, b_rx) = start("win", local(), &[], "token");
    let (_a, a_rx) = start("mac", local(), &[("desktop", b.local_addr())], "token");
    none_within(&a_rx, Duration::from_millis(800), |e| {
        matches!(e, Event::PeerOnline(_))
    });
    none_within(&b_rx, Duration::from_millis(100), is_online("mac"));
}

#[test]
fn reports_offline_when_peer_goes_away() {
    let (b, _b_rx) = start("win", local(), &[], "token");
    let (_a, a_rx) = start("mac", local(), &[("win", b.local_addr())], "token");
    expect(&a_rx, "win 上线", is_online("win"));
    drop(b);
    expect(&a_rx, "win 离线", is_offline("win"));
}

#[test]
fn heartbeats_keep_idle_connection_alive() {
    let (b, _b_rx) = start("win", local(), &[], "token");
    let (_a, a_rx) = start("mac", local(), &[("win", b.local_addr())], "token");
    expect(&a_rx, "win 上线", is_online("win"));
    none_within(&a_rx, Duration::from_millis(1500), is_offline("win"));
}

#[test]
fn simultaneous_dials_settle_on_one_connection() {
    let (a_addr, b_addr) = (free_addr(), free_addr());
    let (a, a_rx) = start("mac", a_addr, &[("win", b_addr)], "token");
    let (b, b_rx) = start("win", b_addr, &[("mac", a_addr)], "token");
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
    let (a, _rx) = start("mac", local(), &[], "token");
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
    let config = LinkConfig::new(info("mac", 0x1B), local(), String::new());
    assert!(matches!(
        TcpPeerLink::start(config, tx),
        Err(Error::Config(_))
    ));
}
