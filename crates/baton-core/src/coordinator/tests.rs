use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::config::{PeerConfig, WakeMethod};
use crate::ports::Waker;

const MAC: InputCode = InputCode(0x1B);
const WIN: InputCode = InputCode(0x0F);

/// 两台机器共用的显示器。`accept_inactive = false` 时只响应当前活跃输入口发来的命令（最坏情况）。
struct Monitor {
    current: Mutex<InputCode>,
    accept_inactive: bool,
}

impl Monitor {
    fn new(current: InputCode, accept_inactive: bool) -> Arc<Self> {
        Arc::new(Self {
            current: Mutex::new(current),
            accept_inactive,
        })
    }

    fn current(&self) -> InputCode {
        *self.current.lock().unwrap()
    }
}

struct FakeDisplay {
    monitor: Arc<Monitor>,
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
        let current = self.monitor.current();
        if current == self.mine || self.monitor.accept_inactive {
            Ok(current)
        } else {
            Err(Error::Ddc("inactive input".into()))
        }
    }
    fn set_input(&self, _: &DisplayId, input: InputCode) -> Result<()> {
        let mut current = self.monitor.current.lock().unwrap();
        if *current == self.mine || self.monitor.accept_inactive {
            *current = input;
            Ok(())
        } else {
            Err(Error::Ddc("inactive input".into()))
        }
    }
}

#[derive(Default)]
struct Probe {
    active_guards: AtomicUsize,
    awake_requests: Mutex<Vec<Duration>>,
    calls: Mutex<Vec<&'static str>>,
    notes: Mutex<Vec<String>>,
    sent: Mutex<Vec<(DeviceId, Message)>>,
    data: Mutex<Vec<(ChannelId, HandoffId, Payload)>>,
    unpacked: Mutex<Vec<Payload>>,
}

impl Probe {
    fn call(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }
    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
    fn guards(&self) -> usize {
        self.active_guards.load(Ordering::SeqCst)
    }
    fn notes(&self) -> String {
        self.notes.lock().unwrap().join("\n")
    }
    fn take_sent(&self) -> Vec<(DeviceId, Message)> {
        std::mem::take(&mut *self.sent.lock().unwrap())
    }
}

struct FakePower(Arc<Probe>);

impl PowerControl for FakePower {
    fn stay_awake(&self, duration: Duration) -> Result<AwakeGuard> {
        self.0.awake_requests.lock().unwrap().push(duration);
        self.0.active_guards.fetch_add(1, Ordering::SeqCst);
        let probe = Arc::clone(&self.0);
        Ok(AwakeGuard::new(move || {
            probe.active_guards.fetch_sub(1, Ordering::SeqCst);
        }))
    }
    fn wake_display(&self) -> Result<()> {
        self.0.call("wake_display");
        Ok(())
    }
    fn restore_sleep(&self) -> Result<()> {
        Ok(())
    }
}

struct FakeInput(Arc<Probe>);

impl InputRouter for FakeInput {
    fn acquire(&self) -> Result<()> {
        self.0.call("acquire");
        Ok(())
    }
    fn release(&self, _: &DeviceId) -> Result<()> {
        self.0.call("release");
        Ok(())
    }
}

struct FakeLink(Arc<Probe>);

impl PeerLink for FakeLink {
    fn send(&self, to: &DeviceId, msg: &Message) -> Result<()> {
        self.0.sent.lock().unwrap().push((to.clone(), msg.clone()));
        Ok(())
    }
    fn send_data(&self, _: &DeviceId, channel: ChannelId, id: HandoffId, p: Payload) -> Result<()> {
        self.0.data.lock().unwrap().push((channel, id, p));
        Ok(())
    }
    fn set_local_state(&self, _: AgentState) {}
}

struct FakeNotifier(Arc<Probe>);

impl Notifier for FakeNotifier {
    fn notify(&self, message: &str) {
        self.0.notes.lock().unwrap().push(message.to_owned());
    }
}

struct FakeWaker(WakeOutcome);

impl Waker for FakeWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::Wol
    }
    fn wake(&self, _: &WakeTarget) -> Result<WakeOutcome> {
        Ok(self.0.clone())
    }
}

struct FakeParticipant {
    probe: Arc<Probe>,
    pack: fn() -> Result<Option<Payload>>,
}

impl HandoffParticipant for FakeParticipant {
    fn channel(&self) -> ChannelId {
        ChannelId::CLIPBOARD
    }
    fn pack(&self, _: &DeviceId) -> Result<Option<Payload>> {
        (self.pack)()
    }
    fn unpack(&self, _: &DeviceId, payload: Payload) -> Result<()> {
        self.probe.unpacked.lock().unwrap().push(payload);
        Ok(())
    }
}

struct Machine {
    id: DeviceId,
    code: InputCode,
    c: Coordinator,
    probe: Arc<Probe>,
}

struct Setup {
    peer: &'static str,
    wake: Option<WakeOutcome>,
    stay_awake_until: StayAwakeUntil,
    pack: fn() -> Result<Option<Payload>>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            peer: "win",
            wake: None,
            stay_awake_until: StayAwakeUntil::PeerOffline,
            pack: || Ok(None),
        }
    }
}

fn clipboard_text() -> Result<Option<Payload>> {
    Ok(Some(Payload {
        format: "text/plain".into(),
        data: b"hello".to_vec(),
    }))
}

fn machine(name: &str, code: InputCode, monitor: &Arc<Monitor>, setup: Setup) -> Machine {
    let probe = Arc::new(Probe::default());
    let mut config = Config::default();
    config.display.input_code = Some(code);
    config.power.stay_awake_until = setup.stay_awake_until;
    config.power.stay_awake_secs = 300;
    config.peers.push(PeerConfig {
        name: setup.peer.into(),
        address: None,
        wol_mac: None,
    });
    let wakers: Vec<Box<dyn Waker>> = setup
        .wake
        .map(|o| Box::new(FakeWaker(o)) as Box<dyn Waker>)
        .into_iter()
        .collect();
    let ports = Ports {
        display: Box::new(FakeDisplay {
            monitor: Arc::clone(monitor),
            mine: code,
        }),
        power: Box::new(FakePower(Arc::clone(&probe))),
        input: Box::new(FakeInput(Arc::clone(&probe))),
        wake: WakeChain::from_config(&[WakeMethod::Wol], wakers),
        link: Arc::new(FakeLink(Arc::clone(&probe))),
        participants: vec![Arc::new(FakeParticipant {
            probe: Arc::clone(&probe),
            pack: setup.pack,
        })],
        notifier: Box::new(FakeNotifier(Arc::clone(&probe))),
    };
    let mut c = Coordinator::new(DeviceId(name.into()), config, ports);
    let _ = c.refresh_state();
    Machine {
        id: DeviceId(name.into()),
        code,
        c,
        probe,
    }
}

fn online(m: &mut Machine, peer: &Machine, now: Instant) {
    m.c.handle(
        Event::PeerOnline(PeerInfo {
            device_id: peer.id.clone(),
            platform: None,
            input_code: Some(peer.code),
            wol_mac: None,
            ip: None,
        }),
        now,
    );
}

/// Mac 持棒、Windows 在线，两边都已握手。
fn pair(monitor: &Arc<Monitor>, mac: Setup, win: Setup) -> (Machine, Machine, Instant) {
    let now = Instant::now();
    let mut a = machine("mac", MAC, monitor, mac);
    let mut b = machine("win", WIN, monitor, Setup { peer: "mac", ..win });
    online(&mut a, &b, now);
    online(&mut b, &a, now);
    (a, b, now)
}

/// 把 `from` 发给 `to` 的消息全部投递过去，返回投递的消息。
fn deliver(from: &Machine, to: &mut Machine, now: Instant) -> Vec<Message> {
    let mut delivered = Vec::new();
    for (dest, message) in from.probe.take_sent() {
        assert_eq!(dest, to.id);
        delivered.push(message.clone());
        to.c.handle(
            Event::Message {
                from: from.id.clone(),
                message,
            },
            now,
        );
    }
    delivered
}

fn kinds(messages: &[Message]) -> Vec<&'static str> {
    messages
        .iter()
        .map(|m| match m {
            Message::Hello { .. }
            | Message::PairCommit { .. }
            | Message::PairNonce { .. }
            | Message::PairReveal { .. }
            | Message::PairPending
            | Message::PairAccept => "handshake",
            Message::Heartbeat { .. } => "heartbeat",
            Message::Prepare { .. } => "prepare",
            Message::Ready { .. } => "ready",
            Message::Taken { .. } => "taken",
            Message::Abort { .. } => "abort",
        })
        .collect()
}

/// 从 `from` 发起，完整走完一次交棒。
fn hand_off(from: &mut Machine, to: &mut Machine, now: Instant) {
    from.c.handle(Event::Hotkey, now);
    assert_eq!(kinds(&deliver(from, to, now)), ["prepare"]);
    assert_eq!(kinds(&deliver(to, from, now)), ["ready"]);
    let poll = now + Timings::default().poll_interval;
    to.c.tick(poll);
    assert_eq!(kinds(&deliver(to, from, poll)), ["taken"]);
}

fn wait_until(cond: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn initial_state_follows_monitor() {
    let monitor = Monitor::new(MAC, false);
    let (a, b, _) = pair(&monitor, Setup::default(), Setup::default());
    assert_eq!(a.c.state(), AgentState::Holding);
    assert_eq!(b.c.state(), AgentState::Idle);
}

#[test]
fn missing_input_code_is_a_config_error() {
    let monitor = Monitor::new(MAC, false);
    let mut m = machine("mac", MAC, &monitor, Setup::default());
    m.c.config.display.input_code = None;
    assert!(matches!(m.c.refresh_state(), Err(Error::Config(_))));
}

#[test]
fn hands_off_both_ways() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());

    hand_off(&mut a, &mut b, now);
    assert_eq!(monitor.current(), WIN);
    assert_eq!(a.c.state(), AgentState::Idle);
    assert_eq!(b.c.state(), AgentState::Holding);
    assert_eq!(a.probe.calls(), ["release"]);
    assert_eq!(b.probe.calls(), ["wake_display", "acquire"]);
    assert_eq!(
        *b.probe.awake_requests.lock().unwrap(),
        [Duration::from_secs(60)]
    );
    assert_eq!(b.probe.guards(), 0, "接棒完成后释放接棒前的短时 guard");

    let later = now + Duration::from_secs(60);
    hand_off(&mut b, &mut a, later);
    assert_eq!(monitor.current(), MAC);
    assert_eq!(a.c.state(), AgentState::Holding);
    assert_eq!(b.c.state(), AgentState::Idle);
    assert_eq!(a.probe.guards(), 0, "重新持棒后释放交棒后的 guard");
    assert_eq!(b.probe.guards(), 1);
}

#[test]
fn stays_awake_until_receiver_goes_offline() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    hand_off(&mut a, &mut b, now);
    assert_eq!(*a.probe.awake_requests.lock().unwrap(), [Duration::MAX]);
    assert_eq!(a.probe.guards(), 1);

    a.c.handle(Event::PeerOffline(b.id.clone()), now);
    assert_eq!(a.probe.guards(), 0);
}

#[test]
fn duration_policy_ignores_peer_offline() {
    let monitor = Monitor::new(MAC, false);
    let mac = Setup {
        stay_awake_until: StayAwakeUntil::Duration,
        ..Setup::default()
    };
    let (mut a, mut b, now) = pair(&monitor, mac, Setup::default());
    hand_off(&mut a, &mut b, now);
    assert_eq!(
        *a.probe.awake_requests.lock().unwrap(),
        [Duration::from_secs(300)]
    );
    a.c.handle(Event::PeerOffline(b.id.clone()), now);
    assert_eq!(a.probe.guards(), 1);
}

#[test]
fn ready_timeout_keeps_holding() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, _b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    assert!(matches!(a.c.state(), AgentState::HandingOff(_)));
    a.probe.take_sent();

    a.c.tick(now + Duration::from_secs(3));
    assert_eq!(a.c.state(), AgentState::Holding);
    assert_eq!(
        kinds(
            &a.probe
                .take_sent()
                .into_iter()
                .map(|(_, m)| m)
                .collect::<Vec<_>>()
        ),
        ["abort"]
    );
    assert!(a.probe.notes().contains("对端没有响应"));
    assert_eq!(monitor.current(), MAC);
}

#[test]
fn taken_timeout_switches_back_when_monitor_allows() {
    let monitor = Monitor::new(MAC, true);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    deliver(&a, &mut b, now);
    deliver(&b, &mut a, now);
    assert_eq!(monitor.current(), WIN);
    assert_eq!(a.probe.guards(), 1);

    a.c.tick(now + Duration::from_secs(8));
    assert_eq!(monitor.current(), MAC);
    assert_eq!(a.c.state(), AgentState::Holding);
    assert_eq!(a.probe.guards(), 0, "交棒失败时释放交棒后的 guard");
    assert_eq!(a.probe.calls(), ["release", "acquire"]);
    assert!(a.probe.notes().contains("已尝试切回本机"));
}

#[test]
fn taken_timeout_without_monitor_access_falls_back_to_idle() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    deliver(&a, &mut b, now);
    deliver(&b, &mut a, now);

    a.c.tick(now + Duration::from_secs(8));
    assert_eq!(monitor.current(), WIN);
    assert_eq!(a.c.state(), AgentState::Idle);
}

#[test]
fn receiver_offline_before_taken_reverts() {
    let monitor = Monitor::new(MAC, true);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    deliver(&a, &mut b, now);
    deliver(&b, &mut a, now);

    a.c.handle(Event::PeerOffline(b.id.clone()), now);
    assert_eq!(monitor.current(), MAC);
    assert_eq!(a.c.state(), AgentState::Holding);
}

#[test]
fn receiver_gives_up_when_picture_never_arrives() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    deliver(&a, &mut b, now);
    b.probe.take_sent();

    b.c.tick(now + Duration::from_secs(8));
    assert_eq!(b.c.state(), AgentState::Idle);
    assert_eq!(b.probe.guards(), 0);
    assert_eq!(
        kinds(
            &b.probe
                .take_sent()
                .into_iter()
                .map(|(_, m)| m)
                .collect::<Vec<_>>()
        ),
        ["abort"]
    );
}

#[test]
fn wakes_offline_peer_then_hands_off() {
    let monitor = Monitor::new(MAC, false);
    let mut a = machine(
        "mac",
        MAC,
        &monitor,
        Setup {
            wake: Some(WakeOutcome::Sent),
            ..Setup::default()
        },
    );
    let b = machine(
        "win",
        WIN,
        &monitor,
        Setup {
            peer: "mac",
            ..Setup::default()
        },
    );
    let now = Instant::now();

    a.c.handle(Event::Hotkey, now);
    assert!(a.probe.take_sent().is_empty());
    assert!(a.probe.notes().contains("已发送唤醒信号"));

    online(&mut a, &b, now + Duration::from_secs(5));
    assert_eq!(
        kinds(
            &a.probe
                .take_sent()
                .into_iter()
                .map(|(_, m)| m)
                .collect::<Vec<_>>()
        ),
        ["prepare"]
    );
}

#[test]
fn wake_wait_times_out() {
    let monitor = Monitor::new(MAC, false);
    let mut a = machine(
        "mac",
        MAC,
        &monitor,
        Setup {
            wake: Some(WakeOutcome::Sent),
            ..Setup::default()
        },
    );
    let now = Instant::now();
    a.c.handle(Event::Hotkey, now);
    a.c.tick(now + Duration::from_secs(30));
    assert!(a.probe.notes().contains("没有在唤醒后上线"));
    assert_eq!(a.c.next_deadline(), None);
}

#[test]
fn needs_user_wake_only_notifies() {
    let monitor = Monitor::new(MAC, false);
    let mut a = machine(
        "mac",
        MAC,
        &monitor,
        Setup {
            wake: Some(WakeOutcome::NeedsUser("请开盖".into())),
            ..Setup::default()
        },
    );
    a.c.handle(Event::Hotkey, Instant::now());
    assert!(a.probe.notes().contains("请开盖"));
    assert_eq!(a.c.next_deadline(), None);
    assert_eq!(a.c.state(), AgentState::Holding);
}

#[test]
fn repeated_hotkey_starts_only_one_handoff() {
    let monitor = Monitor::new(MAC, false);
    let (mut a, _b, now) = pair(&monitor, Setup::default(), Setup::default());
    a.c.handle(Event::Hotkey, now);
    a.c.handle(Event::Hotkey, now);
    assert_eq!(a.probe.take_sent().len(), 1);
}

#[test]
fn idle_machine_ignores_hotkey() {
    let monitor = Monitor::new(MAC, false);
    let (_a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    b.c.handle(Event::Hotkey, now);
    assert!(b.probe.take_sent().is_empty());
    assert_eq!(b.c.state(), AgentState::Idle);
}

#[test]
fn stale_idle_state_is_refreshed_on_hotkey() {
    let monitor = Monitor::new(WIN, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    assert_eq!(b.c.state(), AgentState::Holding);
    *monitor.current.lock().unwrap() = MAC;

    a.c.handle(Event::Hotkey, now);
    assert_eq!(kinds(&deliver(&a, &mut b, now)), ["prepare"]);
}

#[test]
fn receiver_that_holds_rejects_prepare() {
    let monitor = Monitor::new(WIN, false);
    let (mut a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    b.c.handle(
        Event::Message {
            from: a.id.clone(),
            message: Message::Prepare {
                handoff_id: HandoffId(1),
            },
        },
        now,
    );
    assert_eq!(kinds(&deliver(&b, &mut a, now)), ["abort"]);
    assert_eq!(b.c.state(), AgentState::Holding);
}

#[test]
fn participant_data_travels_without_blocking_handoff() {
    let monitor = Monitor::new(MAC, false);
    let mac = Setup {
        pack: clipboard_text,
        ..Setup::default()
    };
    let (mut a, mut b, now) = pair(&monitor, mac, Setup::default());
    hand_off(&mut a, &mut b, now);
    assert!(wait_until(|| !a.probe.data.lock().unwrap().is_empty()));

    let (channel, id, payload) = a.probe.data.lock().unwrap().remove(0);
    assert_eq!(channel, ChannelId::CLIPBOARD);
    b.c.handle(
        Event::Data {
            from: a.id.clone(),
            channel,
            handoff_id: id,
            payload: payload.clone(),
        },
        now,
    );
    assert!(wait_until(|| b.probe.unpacked.lock().unwrap().len() == 1));
    assert_eq!(b.probe.unpacked.lock().unwrap()[0], payload);
}

#[test]
fn failing_participant_does_not_affect_handoff() {
    let monitor = Monitor::new(MAC, false);
    let mac = Setup {
        pack: || Err(Error::Unsupported("剪贴板")),
        ..Setup::default()
    };
    let (mut a, mut b, now) = pair(&monitor, mac, Setup::default());
    hand_off(&mut a, &mut b, now);
    assert_eq!(b.c.state(), AgentState::Holding);
}

#[test]
fn data_of_aborted_handoff_is_discarded() {
    let monitor = Monitor::new(MAC, false);
    let (a, mut b, now) = pair(&monitor, Setup::default(), Setup::default());
    let id = HandoffId(9);
    for message in [
        Message::Prepare { handoff_id: id },
        Message::Abort {
            handoff_id: id,
            reason: "test".into(),
        },
    ] {
        b.c.handle(
            Event::Message {
                from: a.id.clone(),
                message,
            },
            now,
        );
    }
    assert_eq!(b.c.state(), AgentState::Idle);
    b.c.handle(
        Event::Data {
            from: a.id.clone(),
            channel: ChannelId::CLIPBOARD,
            handoff_id: id,
            payload: Payload {
                format: "text/plain".into(),
                data: vec![1],
            },
        },
        now,
    );
    thread::sleep(Duration::from_millis(20));
    assert!(b.probe.unpacked.lock().unwrap().is_empty());
}

struct RecordingWaker(Arc<Mutex<Vec<WakeTarget>>>);

impl Waker for RecordingWaker {
    fn method(&self) -> WakeMethod {
        WakeMethod::Wol
    }
    fn wake(&self, target: &WakeTarget) -> Result<WakeOutcome> {
        self.0.lock().unwrap().push(target.clone());
        Ok(WakeOutcome::Sent)
    }
}

/// 持棒的 Mac，配置里的对端为 `configured`，档案里的设备为 `known`；返回它唤醒过的目标。
fn waking_mac(
    configured: Vec<PeerConfig>,
    known: Vec<PeerInfo>,
) -> (Coordinator, Arc<Mutex<Vec<WakeTarget>>>) {
    let monitor = Monitor::new(MAC, false);
    let probe = Arc::new(Probe::default());
    let woken = Arc::new(Mutex::new(Vec::new()));
    let mut config = Config::default();
    config.display.input_code = Some(MAC);
    config.peers = configured;
    let ports = Ports {
        display: Box::new(FakeDisplay { monitor, mine: MAC }),
        power: Box::new(FakePower(Arc::clone(&probe))),
        input: Box::new(FakeInput(Arc::clone(&probe))),
        wake: WakeChain::from_config(
            &[WakeMethod::Wol],
            vec![Box::new(RecordingWaker(Arc::clone(&woken)))],
        ),
        link: Arc::new(FakeLink(Arc::clone(&probe))),
        participants: Vec::new(),
        notifier: Box::new(FakeNotifier(Arc::clone(&probe))),
    };
    let mut c = Coordinator::new(DeviceId("mac".into()), config, ports).with_known_peers(known);
    c.refresh_state().unwrap();
    (c, woken)
}

fn stored(name: &str, mac: &str, ip: &str) -> PeerInfo {
    PeerInfo {
        device_id: DeviceId(name.into()),
        platform: None,
        input_code: Some(WIN),
        wol_mac: Some(mac.parse().unwrap()),
        ip: Some(ip.parse().unwrap()),
    }
}

#[test]
fn wakes_paired_peer_that_never_came_online() {
    let (mut c, woken) = waking_mac(
        Vec::new(),
        vec![stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20")],
    );
    c.handle(Event::Hotkey, Instant::now());
    assert_eq!(
        woken.lock().unwrap().as_slice(),
        [WakeTarget {
            device_id: DeviceId("win".into()),
            wol_mac: Some("aa:bb:cc:dd:ee:ff".parse().unwrap()),
            last_ip: Some("192.168.1.20".parse().unwrap()),
        }]
    );
}

#[test]
fn reported_mac_takes_precedence_over_config() {
    let (mut c, woken) = waking_mac(
        vec![PeerConfig {
            name: "win".into(),
            address: None,
            wol_mac: Some("11:11:11:11:11:11".parse().unwrap()),
        }],
        vec![stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20")],
    );
    c.handle(Event::Hotkey, Instant::now());
    assert_eq!(
        woken.lock().unwrap()[0].wol_mac,
        Some("aa:bb:cc:dd:ee:ff".parse().unwrap())
    );
}

#[test]
fn config_mac_is_the_fallback() {
    let (mut c, woken) = waking_mac(
        vec![PeerConfig {
            name: "win".into(),
            address: None,
            wol_mac: Some("11:11:11:11:11:11".parse().unwrap()),
        }],
        Vec::new(),
    );
    c.handle(Event::Hotkey, Instant::now());
    assert_eq!(
        woken.lock().unwrap()[0].wol_mac,
        Some("11:11:11:11:11:11".parse().unwrap())
    );
}

#[test]
fn handshake_without_mac_keeps_the_stored_one() {
    let (mut c, woken) = waking_mac(
        Vec::new(),
        vec![stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20")],
    );
    let now = Instant::now();
    c.handle(
        Event::PeerOnline(PeerInfo {
            device_id: DeviceId("win".into()),
            platform: None,
            input_code: Some(WIN),
            wol_mac: None,
            ip: None,
        }),
        now,
    );
    c.handle(Event::PeerOffline(DeviceId("win".into())), now);
    c.handle(Event::Hotkey, now);
    let target = &woken.lock().unwrap()[0];
    assert_eq!(target.wol_mac, Some("aa:bb:cc:dd:ee:ff".parse().unwrap()));
    assert_eq!(target.last_ip, Some("192.168.1.20".parse().unwrap()));
}

#[test]
fn removed_peer_is_no_longer_a_target() {
    let (mut c, woken) = waking_mac(
        Vec::new(),
        vec![stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20")],
    );
    let now = Instant::now();
    c.handle(Event::PeerRemoved(DeviceId("win".into())), now);
    c.handle(Event::Hotkey, now);
    assert!(woken.lock().unwrap().is_empty());
}

#[test]
fn online_known_peer_is_preferred_over_offline_ones() {
    let (mut c, woken) = waking_mac(
        Vec::new(),
        vec![
            stored("alpha", "aa:aa:aa:aa:aa:aa", "192.168.1.21"),
            stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20"),
        ],
    );
    let now = Instant::now();
    c.handle(
        Event::PeerOnline(stored("win", "aa:bb:cc:dd:ee:ff", "192.168.1.20")),
        now,
    );
    c.handle(Event::Hotkey, now);
    assert!(woken.lock().unwrap().is_empty(), "在线的对端不需要唤醒");
    assert!(matches!(c.state(), AgentState::HandingOff(_)));
}
