//! 交棒协调器：由 [`Event`] 和时钟驱动的状态机，流程见设计文档第 6 节。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::channel::ChannelId;
use crate::config::{Config, DisplayMode, StayAwakeUntil};
use crate::event::{Event, PeerInfo};
use crate::ids::{DeviceId, DisplayId, HandoffId, InputCode, MacAddr};
use crate::participant::{HandoffParticipant, Payload};
use crate::ports::{
    AwakeGuard, DisplayControl, DisplayInfo, InputRouter, Notifier, PeerLink, PowerControl,
    WakeOutcome, WakeTarget,
};
use crate::protocol::Message;
use crate::state::AgentState;
use crate::wake::WakeChain;
use crate::{Error, Result};

/// 注入给协调器的全部外部能力。
pub struct Ports {
    pub display: Box<dyn DisplayControl>,
    pub power: Box<dyn PowerControl>,
    pub input: Box<dyn InputRouter>,
    pub wake: WakeChain,
    pub link: Arc<dyn PeerLink>,
    pub participants: Vec<Arc<dyn HandoffParticipant>>,
    pub notifier: Box<dyn Notifier>,
}

/// 流程中的固定时长；交棒确认超时取自配置 `handoff_timeout_ms`。
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    /// 唤醒信号发出后等待对端上线的时长。
    pub wake_wait: Duration,
    /// 发出 `prepare` 后等待 `ready` 的时长。
    pub ready_timeout: Duration,
    /// 接棒方轮询显示器输入源的间隔。
    pub poll_interval: Duration,
    /// 接棒方收到 `prepare` 后保持唤醒的时长，防止合盖的 Mac 在画面切过来前睡回去。
    pub receiver_awake: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            wake_wait: Duration::from_secs(30),
            ready_timeout: Duration::from_secs(3),
            poll_interval: Duration::from_millis(300),
            receiver_awake: Duration::from_secs(60),
        }
    }
}

/// 进行中的交棒步骤，每一步都有截止时间。
enum Pending {
    Waking {
        target: DeviceId,
        deadline: Instant,
    },
    AwaitReady {
        id: HandoffId,
        target: DeviceId,
        deadline: Instant,
    },
    AwaitTaken {
        id: HandoffId,
        target: DeviceId,
        deadline: Instant,
        awake: Option<AwakeGuard>,
    },
    Receiving {
        id: HandoffId,
        from: DeviceId,
        deadline: Instant,
        next_poll: Instant,
        awake: Option<AwakeGuard>,
    },
}

pub struct Coordinator {
    me: DeviceId,
    config: Config,
    ports: Ports,
    timings: Timings,
    state: AgentState,
    known: HashMap<DeviceId, PeerInfo>,
    online: HashSet<DeviceId>,
    /// 本机输入源编码：配置值，或自动学到的值。
    my_code: Option<InputCode>,
    pending: Option<Pending>,
    /// 交出控制权后保持唤醒的 guard；`peer_offline` 策略下在该对端离线时释放。
    awake_after_handoff: Option<(DeviceId, AwakeGuard)>,
    /// 已中止的接棒，迟到的随身数据直接丢弃。
    discarded: Option<HandoffId>,
}

impl Coordinator {
    pub fn new(me: DeviceId, config: Config, ports: Ports) -> Self {
        Self {
            me,
            my_code: config.display.input_code,
            config,
            ports,
            timings: Timings::default(),
            state: AgentState::Idle,
            known: HashMap::new(),
            online: HashSet::new(),
            pending: None,
            awake_after_handoff: None,
            discarded: None,
        }
    }

    pub fn with_timings(mut self, timings: Timings) -> Self {
        self.timings = timings;
        self
    }

    /// 设备档案里的已配对设备：从未上线也能据此选为交棒目标并唤醒。
    pub fn with_known_peers(mut self, peers: impl IntoIterator<Item = PeerInfo>) -> Self {
        for peer in peers {
            self.known.insert(peer.device_id.clone(), peer);
        }
        self
    }

    /// 本机输入源编码的初始值（配置值或上次学到的值）。
    pub fn with_input_code(mut self, code: Option<InputCode>) -> Self {
        self.my_code = code;
        self
    }

    pub fn config(&self) -> &Config {
        &self.config
    }

    pub fn state(&self) -> AgentState {
        self.state
    }

    #[cfg(test)]
    pub fn device_id(&self) -> &DeviceId {
        &self.me
    }

    /// 通过读取显示器当前输入源判断本机是否持棒：读到本机的输入源编码即为持棒。
    pub fn refresh_state(&mut self) -> Result<AgentState> {
        let mine = self.my_input_code()?;
        let display = self.display_id()?;
        let state = match self.ports.display.get_input(&display) {
            Ok(current) if current == mine => AgentState::Holding,
            _ => AgentState::Idle,
        };
        self.set_state(state);
        Ok(state)
    }

    /// 处理事件直到收到 [`Event::Shutdown`] 或事件源关闭。
    pub fn run(mut self, events: Receiver<Event>) {
        if let Err(e) = self.refresh_state() {
            if self.my_code.is_none() && self.config.display.mode == DisplayMode::Ddc {
                log::info!("本机接口尚未识别，在本机操作一次键鼠后自动识别");
            } else {
                self.notify(&format!("无法判定持棒状态：{e}"));
            }
        }
        log::info!("{} 启动，状态 {:?}", self.me.0, self.state);
        loop {
            let event = match self.next_deadline() {
                Some(deadline) => {
                    match events.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(event) => Some(event),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                None => match events.recv() {
                    Ok(event) => Some(event),
                    Err(_) => break,
                },
            };
            let now = Instant::now();
            match event {
                Some(Event::Shutdown) => break,
                Some(event) => self.handle(event, now),
                None => {}
            }
            self.tick(now);
        }
    }

    pub fn handle(&mut self, event: Event, now: Instant) {
        match event {
            Event::Hotkey => self.on_handoff_request(None, now),
            Event::HandoffTo(target) => self.on_handoff_request(Some(target), now),
            Event::LocalInputCode(code) => self.on_local_input_code(code),
            Event::LocalName(name) => self.me = name,
            Event::PeerOnline(info) => self.on_peer_online(info, now),
            Event::PeerUpdated(info) => self.on_peer_updated(info),
            Event::PeerOffline(peer) => self.on_peer_offline(&peer),
            Event::PeerRemoved(peer) => {
                self.on_peer_offline(&peer);
                self.known.remove(&peer);
            }
            Event::Message { from, message } => self.on_message(from, message, now),
            Event::Data {
                from,
                channel,
                handoff_id,
                payload,
            } => self.on_data(from, channel, handoff_id, payload),
            Event::Shutdown => {}
        }
    }

    /// 处理到期的超时和轮询。
    pub fn tick(&mut self, now: Instant) {
        match self.pending.take() {
            Some(Pending::Waking { target, deadline }) if now >= deadline => {
                self.notify(&format!("{} 没有在唤醒后上线，请手动唤醒", target.0));
            }
            Some(Pending::AwaitReady {
                id,
                target,
                deadline,
            }) if now >= deadline => {
                self.abort_before_switch(&target, id, "对端没有响应");
            }
            Some(Pending::AwaitTaken {
                id,
                target,
                deadline,
                ..
            }) if now >= deadline => {
                self.revert(&target, id, "对端没有确认接棒", true);
            }
            Some(Pending::Receiving {
                id,
                from,
                deadline,
                next_poll,
                awake,
            }) if now >= next_poll => {
                if self.shows_my_input() {
                    if let Err(e) = self.ports.input.acquire() {
                        self.notify(&format!("接管键鼠失败：{e}"));
                    }
                    self.send(&from, Message::Taken { handoff_id: id });
                    self.set_state(AgentState::Holding);
                    self.awake_after_handoff = None;
                    log::info!("已从 {} 接棒", from.0);
                } else if now >= deadline {
                    self.send(
                        &from,
                        Message::Abort {
                            handoff_id: id,
                            reason: "接棒方超时未看到画面切换".into(),
                        },
                    );
                    self.discarded = Some(id);
                    self.set_state(AgentState::Idle);
                } else {
                    self.pending = Some(Pending::Receiving {
                        id,
                        from,
                        deadline,
                        next_poll: now + self.timings.poll_interval,
                        awake,
                    });
                }
            }
            other => self.pending = other,
        }
    }

    /// 下一次需要调用 [`Self::tick`] 的时间。
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.pending.as_ref()? {
            Pending::Waking { deadline, .. }
            | Pending::AwaitReady { deadline, .. }
            | Pending::AwaitTaken { deadline, .. } => Some(*deadline),
            Pending::Receiving {
                deadline,
                next_poll,
                ..
            } => Some((*next_poll).min(*deadline)),
        }
    }

    /// 热键（`target` 为空，自动选目标）或面板上点了某台电脑。
    fn on_handoff_request(&mut self, target: Option<DeviceId>, now: Instant) {
        if self.pending.is_some() {
            log::info!("交棒进行中，忽略本次请求");
            return;
        }
        if self.state != AgentState::Holding {
            // 判定可能已过时（例如上次交棒超时后画面才切过来），按显示器实际输入源重新判定。
            if let Err(e) = self.refresh_state() {
                self.notify(&format!("无法判定持棒状态：{e}"));
                return;
            }
            if self.state != AgentState::Holding {
                if target.is_some() {
                    self.notify("画面不在本机，请在持有画面的电脑上操作");
                } else {
                    log::info!("本机不持棒，忽略热键");
                }
                return;
            }
        }
        let target = match target {
            Some(target) if self.known.contains_key(&target) || self.online.contains(&target) => {
                target
            }
            Some(target) => {
                self.notify(&format!("{} 不是已配对的设备", target.0));
                return;
            }
            None => match self.pick_target() {
                Some(target) => target,
                None => {
                    self.notify("没有可交棒的对端，请先在 DeskBaton 面板中配对设备");
                    return;
                }
            },
        };
        if self.online.contains(&target) {
            self.start_prepare(target, now);
            return;
        }
        let wake_target = WakeTarget {
            wol_mac: self.wol_mac(&target),
            last_ip: self.known.get(&target).and_then(|p| p.ip),
            device_id: target.clone(),
        };
        match self.ports.wake.wake(&wake_target) {
            Ok(WakeOutcome::Sent) => {
                self.notify(&format!("{} 不在线，已发送唤醒信号", target.0));
                self.pending = Some(Pending::Waking {
                    target,
                    deadline: now + self.timings.wake_wait,
                });
            }
            Ok(WakeOutcome::NeedsUser(message)) => self.notify(&message),
            Err(e) => self.notify(&format!("{} 不在线，且无法唤醒：{e}", target.0)),
        }
    }

    fn on_peer_online(&mut self, info: PeerInfo, now: Instant) {
        let peer = info.device_id.clone();
        log::info!("{} 上线", peer.0);
        self.online.insert(peer.clone());
        let merged = match self.known.remove(&peer) {
            Some(old) => PeerInfo {
                platform: info.platform.or(old.platform),
                input_code: info.input_code.or(old.input_code),
                wol_mac: info.wol_mac.or(old.wol_mac),
                ip: info.ip.or(old.ip),
                ..info
            },
            None => info,
        };
        self.known.insert(peer.clone(), merged);
        if matches!(&self.pending, Some(Pending::Waking { target, .. }) if *target == peer) {
            self.pending = None;
            self.start_prepare(peer, now);
        }
    }

    fn on_local_input_code(&mut self, code: Option<InputCode>) {
        log::info!(
            "本机输入源编码：{}",
            code.map_or_else(|| "未识别".to_owned(), |c| c.to_string())
        );
        self.my_code = code;
        if self.pending.is_none() && self.refresh_state().is_err() {
            self.set_state(AgentState::Idle);
        }
    }

    fn on_peer_updated(&mut self, info: PeerInfo) {
        let Some(old) = self.known.get_mut(&info.device_id) else {
            self.known.insert(info.device_id.clone(), info);
            return;
        };
        old.platform = info.platform.or(old.platform);
        old.input_code = info.input_code;
        old.wol_mac = info.wol_mac.or(old.wol_mac);
        old.ip = info.ip.or(old.ip);
    }

    fn on_peer_offline(&mut self, peer: &DeviceId) {
        log::info!("{} 离线", peer.0);
        self.online.remove(peer);
        if self.config.power.stay_awake_until == StayAwakeUntil::PeerOffline
            && matches!(&self.awake_after_handoff, Some((holder, _)) if holder == peer)
        {
            self.awake_after_handoff = None;
            log::info!("接棒方离线，恢复系统睡眠");
        }
        match self.pending.take() {
            Some(Pending::AwaitReady { id, target, .. }) if target == *peer => {
                self.abort_before_switch(&target, id, "对端离线");
            }
            Some(Pending::AwaitTaken { id, target, .. }) if target == *peer => {
                self.revert(&target, id, "对端在确认接棒前离线", false);
            }
            Some(Pending::Receiving { id, from, .. }) if from == *peer => {
                self.discarded = Some(id);
                self.set_state(AgentState::Idle);
            }
            other => self.pending = other,
        }
    }

    fn on_message(&mut self, from: DeviceId, message: Message, now: Instant) {
        match message {
            Message::Prepare { handoff_id } => self.on_prepare(from, handoff_id, now),
            Message::Ready { handoff_id } => self.on_ready(from, handoff_id, now),
            Message::Taken { handoff_id } => self.on_taken(&from, handoff_id),
            Message::Abort { handoff_id, reason } => self.on_abort(&from, handoff_id, &reason),
            Message::Hello { .. }
            | Message::PairCommit { .. }
            | Message::PairNonce { .. }
            | Message::PairReveal { .. }
            | Message::PairPending
            | Message::PairAccept
            | Message::Heartbeat { .. }
            | Message::InputCode { .. } => {}
        }
    }

    fn start_prepare(&mut self, target: DeviceId, now: Instant) {
        if self.known.get(&target).and_then(|p| p.input_code).is_none() {
            self.notify(&format!(
                "{0} 还没识别接口：先用显示器菜单切到 {0}，在它上面操作一次键鼠",
                target.0
            ));
            return;
        }
        let id = new_handoff_id();
        if let Err(e) = self
            .ports
            .link
            .send(&target, &Message::Prepare { handoff_id: id })
        {
            self.notify(&format!("无法联系 {}：{e}", target.0));
            return;
        }
        self.set_state(AgentState::HandingOff(id));
        self.pending = Some(Pending::AwaitReady {
            id,
            target,
            deadline: now + self.timings.ready_timeout,
        });
    }

    /// 接棒方：点亮显示、短时保持唤醒，然后轮询显示器直到画面切过来。
    fn on_prepare(&mut self, from: DeviceId, id: HandoffId, now: Instant) {
        if matches!(
            self.pending,
            Some(Pending::Waking { .. } | Pending::AwaitReady { .. } | Pending::AwaitTaken { .. })
        ) {
            self.send_abort(&from, id, "接棒方正在交棒");
            return;
        }
        if self.state == AgentState::Holding {
            let _ = self.refresh_state();
            if self.state == AgentState::Holding {
                self.send_abort(&from, id, "接棒方当前持棒");
                return;
            }
        }
        let awake = match self.ports.power.stay_awake(self.timings.receiver_awake) {
            Ok(guard) => Some(guard),
            Err(e) => {
                log::warn!("接棒前保持唤醒失败：{e}");
                None
            }
        };
        if let Err(e) = self.ports.power.wake_display() {
            log::warn!("点亮显示失败：{e}");
        }
        if let Err(e) = self
            .ports
            .link
            .send(&from, &Message::Ready { handoff_id: id })
        {
            log::warn!("回复 ready 失败：{e}");
            return;
        }
        self.discarded = None;
        self.set_state(AgentState::Receiving(id));
        self.pending = Some(Pending::Receiving {
            id,
            from,
            deadline: now + self.handoff_timeout(),
            next_poll: now + self.timings.poll_interval,
            awake,
        });
    }

    /// 持棒方：交出键鼠、保持唤醒、送出随身数据，最后切换输入源。
    fn on_ready(&mut self, from: DeviceId, id: HandoffId, now: Instant) {
        if !matches!(&self.pending, Some(Pending::AwaitReady { id: pending, target, .. })
            if *pending == id && *target == from)
        {
            return;
        }
        self.pending = None;
        let Some(target_code) = self.known.get(&from).and_then(|p| p.input_code) else {
            self.abort_before_switch(&from, id, "对端没有声明输入源编码");
            return;
        };
        if let Err(e) = self.ports.input.release(&from) {
            self.abort_before_switch(&from, id, &format!("交出键鼠失败：{e}"));
            return;
        }
        let awake = self.stay_awake_after_handoff();
        self.send_participant_data(&from, id);
        if let Err(e) = self.switch_input(target_code) {
            let _ = self.ports.input.acquire();
            self.abort_before_switch(&from, id, &format!("切换输入源失败：{e}"));
            return;
        }
        self.pending = Some(Pending::AwaitTaken {
            id,
            target: from,
            deadline: now + self.handoff_timeout(),
            awake,
        });
    }

    fn on_taken(&mut self, from: &DeviceId, id: HandoffId) {
        match self.pending.take() {
            Some(Pending::AwaitTaken {
                id: pending,
                target,
                awake,
                ..
            }) if pending == id && target == *from => {
                self.set_state(AgentState::Idle);
                self.awake_after_handoff = awake.map(|guard| (target.clone(), guard));
                log::info!("已交棒给 {}", target.0);
            }
            other => self.pending = other,
        }
    }

    fn on_abort(&mut self, from: &DeviceId, id: HandoffId, reason: &str) {
        match self.pending.take() {
            Some(Pending::AwaitReady {
                id: pending,
                target,
                ..
            }) if pending == id && target == *from => {
                self.set_state(AgentState::Holding);
                self.notify(&format!("{} 放弃接棒：{reason}", target.0));
            }
            Some(Pending::AwaitTaken {
                id: pending,
                target,
                ..
            }) if pending == id && target == *from => {
                self.revert(&target, id, reason, false);
            }
            Some(Pending::Receiving {
                id: pending,
                from: holder,
                ..
            }) if pending == id && holder == *from => {
                self.discarded = Some(id);
                self.set_state(AgentState::Idle);
                log::info!("{} 中止交棒：{reason}", holder.0);
            }
            other => self.pending = other,
        }
    }

    fn on_data(&self, from: DeviceId, channel: ChannelId, id: HandoffId, payload: Payload) {
        if self.discarded == Some(id) {
            log::info!("交棒已中止，丢弃通道 {} 的数据", channel.0);
            return;
        }
        let Some(participant) = self
            .ports
            .participants
            .iter()
            .find(|p| p.channel() == channel)
            .cloned()
        else {
            log::warn!("通道 {} 没有对应的交棒参与者", channel.0);
            return;
        };
        thread::spawn(move || {
            if let Err(e) = participant.unpack(&from, payload) {
                log::warn!("通道 {} 解包失败：{e}", channel.0);
            }
        });
    }

    /// 切换输入源之前失败：本机仍然持棒。
    fn abort_before_switch(&mut self, target: &DeviceId, id: HandoffId, reason: &str) {
        self.send_abort(target, id, reason);
        self.pending = None;
        self.set_state(AgentState::Holding);
        self.notify(&format!("交棒失败：{reason}"));
    }

    /// 输入源可能已经切走：尽力切回本机，再按显示器实际状态判定是否持棒。
    fn revert(&mut self, target: &DeviceId, id: HandoffId, reason: &str, tell_peer: bool) {
        self.pending = None;
        if tell_peer {
            self.send_abort(target, id, reason);
        }
        if let Err(e) = self
            .my_input_code()
            .and_then(|code| self.switch_input(code))
        {
            log::warn!("切回本机失败：{e}");
        }
        if let Err(e) = self.ports.input.acquire() {
            log::warn!("收回键鼠失败：{e}");
        }
        if self.refresh_state().is_err() {
            self.set_state(AgentState::Idle);
        }
        self.notify(&format!("交棒失败：{reason}，已尝试切回本机"));
    }

    fn stay_awake_after_handoff(&self) -> Option<AwakeGuard> {
        let duration = match self.config.power.stay_awake_until {
            StayAwakeUntil::PeerOffline => Duration::MAX,
            StayAwakeUntil::Duration => Duration::from_secs(self.config.power.stay_awake_secs),
        };
        match self.ports.power.stay_awake(duration) {
            Ok(guard) => Some(guard),
            Err(e) => {
                log::warn!("保持唤醒失败，继续交棒：{e}");
                None
            }
        }
    }

    /// 每个参与者在独立线程里打包并排队发送，不阻塞交棒。
    fn send_participant_data(&self, to: &DeviceId, id: HandoffId) {
        for participant in &self.ports.participants {
            let participant = Arc::clone(participant);
            let link = Arc::clone(&self.ports.link);
            let to = to.clone();
            thread::spawn(move || {
                let channel = participant.channel();
                match participant.pack(&to) {
                    Ok(Some(payload)) => {
                        if let Err(e) = link.send_data(&to, channel, id, payload) {
                            log::warn!("通道 {} 发送失败：{e}", channel.0);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => log::warn!("通道 {} 打包失败：{e}", channel.0),
                }
            });
        }
    }

    /// 配置里的对端优先（按配置顺序），其次档案与握手得知的设备（按名称）；在线的先。
    fn pick_target(&self) -> Option<DeviceId> {
        let mut candidates: Vec<DeviceId> = self
            .config
            .peers
            .iter()
            .map(|p| DeviceId(p.name.clone()))
            .collect();
        let mut others: Vec<DeviceId> = self
            .known
            .keys()
            .chain(&self.online)
            .filter(|p| !candidates.contains(p))
            .cloned()
            .collect();
        others.sort();
        others.dedup();
        candidates.extend(others);
        candidates
            .iter()
            .find(|p| self.online.contains(*p))
            .or_else(|| candidates.first())
            .cloned()
    }

    /// 握手上报或档案记下的 MAC 优先，配置只作兜底。
    fn wol_mac(&self, peer: &DeviceId) -> Option<MacAddr> {
        self.known.get(peer).and_then(|p| p.wol_mac).or_else(|| {
            self.config
                .peers
                .iter()
                .find(|p| p.name == peer.0)
                .and_then(|p| p.wol_mac)
        })
    }

    fn shows_my_input(&self) -> bool {
        let (Ok(mine), Ok(display)) = (self.my_input_code(), self.display_id()) else {
            return false;
        };
        matches!(self.ports.display.get_input(&display), Ok(current) if current == mine)
    }

    fn switch_input(&self, code: InputCode) -> Result<()> {
        let display = self.display_id()?;
        self.ports.display.set_input(&display, code)
    }

    fn my_input_code(&self) -> Result<InputCode> {
        match self.config.display.mode {
            DisplayMode::Ddc => self
                .my_code
                .ok_or_else(|| Error::Config("本机还没识别接口，请先在本机操作一次键鼠".into())),
            DisplayMode::None => Err(Error::Unsupported("不切屏模式的持棒判定")),
        }
    }

    fn display_id(&self) -> Result<DisplayId> {
        Ok(DisplayInfo::select(self.ports.display.list()?, &self.config.display.matcher)?.id)
    }

    fn handoff_timeout(&self) -> Duration {
        Duration::from_millis(self.config.handoff_timeout_ms)
    }

    fn set_state(&mut self, state: AgentState) {
        self.state = state;
        self.ports.link.set_local_state(state);
    }

    fn send(&self, to: &DeviceId, message: Message) {
        if let Err(e) = self.ports.link.send(to, &message) {
            log::warn!("发送给 {} 失败：{e}", to.0);
        }
    }

    fn send_abort(&self, to: &DeviceId, id: HandoffId, reason: &str) {
        self.send(
            to,
            Message::Abort {
                handoff_id: id,
                reason: reason.to_owned(),
            },
        );
    }

    fn notify(&self, message: &str) {
        log::warn!("{message}");
        self.ports.notifier.notify(message);
    }
}

fn new_handoff_id() -> HandoffId {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    HandoffId(nanos)
}

#[cfg(test)]
mod tests;
