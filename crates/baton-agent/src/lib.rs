//! 组装各平台实现，启动链路与交棒协调器。无界面的 `deskbaton` 与面板 `deskbaton-gui` 共用。

pub mod hotkey;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use baton_core::config::{Config, DisplayMode};
use baton_core::coordinator::{Coordinator, Ports};
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::{DeviceId, InputCode, Platform};
use baton_core::learn::InputCodeLearner;
use baton_core::ports::{DisplayInfo, Notifier};
use baton_core::state_file::LocalState;
use baton_core::wake::WakeChain;
use baton_core::{Error, Result};
use baton_net::{
    LinkConfig, LinkObserver, PairedPeer, PeerAddr, PeerStore, TcpPeerLink, TlsIdentity,
};

pub struct Options {
    pub config: Config,
    /// 本机身份 `identity/` 与设备档案 `peers.toml` 所在目录，即配置文件所在目录。
    pub data_dir: PathBuf,
    pub notifier: Box<dyn Notifier>,
    /// 配对界面；为 `None` 时未配对的设备一律拒绝。
    pub observer: Option<Arc<dyn LinkObserver>>,
    /// 协调器线程结束后（收到 [`Event::Shutdown`]）在该线程上调用。
    pub on_stopped: Box<dyn FnOnce() + Send>,
}

pub struct Agent {
    me: DeviceId,
    hotkey: String,
    events: Sender<Event>,
    link: Arc<TcpPeerLink>,
    learner: Arc<Mutex<InputCodeLearner>>,
    state_path: PathBuf,
    coordinator: Option<JoinHandle<()>>,
    /// 丢弃即通知学习线程退出。
    stop_learning: Option<Sender<()>>,
    learning: Option<JoinHandle<()>>,
}

impl Agent {
    /// 恢复上次残留的睡眠设置，加载身份、档案与本机状态，开始监听、发现、拨号，
    /// 并在独立线程运行协调器和输入源学习。
    pub fn start(options: Options) -> Result<Self> {
        let Options {
            config,
            data_dir,
            notifier,
            observer,
            on_stopped,
        } = options;
        let power = baton_power::platform_power();
        if let Err(e) = power.restore_sleep() {
            log::warn!("恢复残留的睡眠设置失败：{e}");
        }
        let me = DeviceId(config.name.clone().unwrap_or_else(hostname));
        let store = Arc::new(PeerStore::open(&data_dir.join("peers.toml"))?);
        let known: Vec<PeerInfo> = store.list().iter().map(PairedPeer::info).collect();
        let identity = Arc::new(TlsIdentity::load_or_create(&data_dir.join("identity"))?);
        let state_path = data_dir.join("state.toml");
        let learned = LocalState::load(&state_path)
            .inspect_err(|e| log::warn!("读取本机状态失败，按未识别处理：{e}"))
            .unwrap_or_default()
            .input_code;
        let learner = Arc::new(Mutex::new(InputCodeLearner::new(
            config.display.input_code,
            learned,
        )));
        let my_code = lock(&learner).current();

        let (events, events_rx) = mpsc::channel();
        let mut link_config = link_config(&me, &config, identity, store);
        link_config.me.input_code = my_code;
        link_config.observer = observer;
        let link = Arc::new(TcpPeerLink::start(link_config, events.clone())?);
        log::info!(
            "{} 监听 {}，热键 {}，数据目录 {}",
            me.0,
            link.local_addr(),
            config.hotkey,
            data_dir.display()
        );

        let hotkey = config.hotkey.clone();
        let (stop_learning, learning) = if config.display.mode == DisplayMode::Ddc {
            let (stop, stopped) = mpsc::channel();
            let learning = Learning {
                learner: Arc::clone(&learner),
                matcher: config.display.matcher.clone(),
                state_path: state_path.clone(),
                events: events.clone(),
                link: Arc::clone(&link),
            };
            let handle = thread::Builder::new()
                .name("input-learning".into())
                .spawn(move || learning.run(&stopped))
                .map_err(|e| Error::Config(format!("无法启动输入源学习线程：{e}")))?;
            (Some(stop), Some(handle))
        } else {
            (None, None)
        };
        let ports = Ports {
            display: baton_display::platform_display(),
            power,
            input: baton_input::router_for(&config.input),
            wake: WakeChain::from_config(&config.power.wake_chain, baton_power::available_wakers()),
            link: Arc::clone(&link) as Arc<dyn baton_core::ports::PeerLink>,
            participants: Vec::new(),
            notifier,
        };
        let coordinator = Coordinator::new(me.clone(), config, ports)
            .with_known_peers(known)
            .with_input_code(my_code);
        let coordinator = thread::Builder::new()
            .name("coordinator".into())
            .spawn(move || {
                coordinator.run(events_rx);
                log::info!("协调器已退出");
                on_stopped();
            })
            .map_err(|e| Error::Config(format!("无法启动协调器线程：{e}")))?;
        Ok(Self {
            me,
            hotkey,
            events,
            link,
            learner,
            state_path,
            coordinator: Some(coordinator),
            stop_learning,
            learning,
        })
    }

    /// 本机输入源编码：配置值，或自动学到的值。
    pub fn local_input_code(&self) -> Option<InputCode> {
        lock(&self.learner).current()
    }

    /// 配置文件写了 `display.input_code`，不需要也不能重新识别。
    pub fn input_code_configured(&self) -> bool {
        lock(&self.learner).is_configured()
    }

    /// 忘掉学到的本机编码，下次本机有键鼠输入时重新学习。
    pub fn relearn_input(&self) -> Result<()> {
        if !lock(&self.learner).reset() {
            return Err(Error::Config(
                "配置文件写了 display.input_code，以配置为准".into(),
            ));
        }
        LocalState::default().save(&self.state_path)?;
        let _ = self.events.send(Event::LocalInputCode(None));
        self.link.set_local_input_code(None);
        Ok(())
    }

    pub fn me(&self) -> &DeviceId {
        &self.me
    }

    pub fn hotkey(&self) -> &str {
        &self.hotkey
    }

    /// 热键、`--stdin` 等外部输入经它送入协调器。
    pub fn events(&self) -> Sender<Event> {
        self.events.clone()
    }

    pub fn link(&self) -> &Arc<TcpPeerLink> {
        &self.link
    }

    /// 通知协调器退出并等它结束，保证 `AwakeGuard` 已释放。可重复调用。
    pub fn shutdown(&mut self) {
        self.stop_learning = None;
        if let Some(learning) = self.learning.take()
            && learning.join().is_err()
        {
            log::warn!("输入源学习线程异常退出");
        }
        let _ = self.events.send(Event::Shutdown);
        if let Some(coordinator) = self.coordinator.take()
            && coordinator.join().is_err()
        {
            log::warn!("协调器线程异常退出");
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 学习线程：每隔 [`LEARN_INTERVAL`] 检查一次键鼠活动，学到本机编码后通知协调器与对端。
struct Learning {
    learner: Arc<Mutex<InputCodeLearner>>,
    matcher: String,
    state_path: PathBuf,
    events: Sender<Event>,
    link: Arc<TcpPeerLink>,
}

const LEARN_INTERVAL: Duration = Duration::from_secs(2);

impl Learning {
    fn run(self, stopped: &Receiver<()>) {
        let display = baton_display::platform_display();
        let activity = baton_input::PlatformActivity;
        while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(LEARN_INTERVAL) {
            let learned = lock(&self.learner).poll(&activity, || {
                let id = DisplayInfo::select(display.list()?, &self.matcher)?.id;
                display.get_input(&id)
            });
            let Some(code) = learned else {
                continue;
            };
            log::info!("已识别本机输入源编码 {code}");
            let state = LocalState {
                input_code: Some(code),
            };
            if let Err(e) = state.save(&self.state_path) {
                log::warn!("保存本机输入源编码失败，下次启动需要重新识别：{e}");
            }
            let _ = self.events.send(Event::LocalInputCode(Some(code)));
            self.link.set_local_input_code(Some(code));
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn link_config(
    me: &DeviceId,
    config: &Config,
    identity: Arc<TlsIdentity>,
    store: Arc<PeerStore>,
) -> LinkConfig {
    let info = PeerInfo {
        device_id: me.clone(),
        platform: Some(Platform::current()),
        input_code: config.display.input_code,
        wol_mac: None,
        ip: None,
    };
    let listen = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.network.port));
    let mut link = LinkConfig::new(info, listen, identity, store);
    link.token.clone_from(&config.network.token);
    link.discovery = true;
    link.peers = config
        .peers
        .iter()
        .filter_map(|p| {
            Some(PeerAddr {
                device_id: DeviceId(p.name.clone()),
                address: p.address.clone()?,
            })
        })
        .collect();
    link
}

/// 读取配置文件；`missing_ok` 时文件不存在则用默认配置。
pub fn load_config(path: &Path, missing_ok: bool) -> Result<Config> {
    match std::fs::read_to_string(path) {
        Ok(text) => Config::from_toml_str(&text)
            .map_err(|e| Error::Config(format!("配置文件 {} 有误：{e}", path.display()))),
        Err(e) if missing_ok && e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(e) => Err(Error::Config(format!(
            "无法读取配置文件 {}：{e}。可参考仓库中的 config.example.toml",
            path.display()
        ))),
    }
}

/// 配置文件所在目录即数据目录。
pub fn data_dir_of(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn hostname() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    name.trim_end_matches(".local").to_owned()
}
