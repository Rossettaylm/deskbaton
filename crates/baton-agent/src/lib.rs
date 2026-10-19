//! 组装各平台实现，启动链路与交棒协调器。无界面的 `deskbaton` 与面板 `deskbaton-gui` 共用。

pub mod hotkey;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use baton_clipboard::{ClipboardBridge, ClipboardSync, EchoSet, SystemClipboard};
use baton_core::channel::ChannelId;
use baton_core::config::{Config, DisplayMode};
use baton_core::coordinator::{Coordinator, Ports};
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::{DeviceId, HandoffId, InputCode, Platform};
use baton_core::learn::InputCodeLearner;
use baton_core::outbox::PeerRef;
use baton_core::ports::{ClipboardAccess, DisplayInfo, Notifier, PeerLink};
use baton_core::state_file::{LocalState, clamp_outbox_limit};
use baton_core::wake::WakeChain;
use baton_core::{Error, Result};
use baton_files::{FileObserver, FileService, FileSettings};
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
    /// 文件快传界面；为 `None` 时「需要确认」的请求一律拒绝。
    pub file_observer: Option<Arc<dyn FileObserver>>,
    /// 协调器线程结束后（收到 [`Event::Shutdown`]）在该线程上调用。
    pub on_stopped: Box<dyn FnOnce() + Send>,
}

pub struct Agent {
    me: DeviceId,
    display_mode: DisplayMode,
    hotkey: String,
    events: Sender<Event>,
    link: Arc<TcpPeerLink>,
    learner: Arc<Mutex<InputCodeLearner>>,
    state_path: PathBuf,
    coordinator: Option<JoinHandle<()>>,
    /// 丢弃即通知学习线程退出。
    stop_learning: Option<Sender<()>>,
    learning: Option<JoinHandle<()>>,
    clipboard_sync: Arc<AtomicBool>,
    clipboard: Arc<ClipboardBridge>,
    /// 丢弃即通知剪贴板线程退出。
    stop_clipboard: Option<Sender<()>>,
    clipboard_thread: Option<JoinHandle<()>>,
    files: Arc<FileService>,
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
            file_observer,
            on_stopped,
        } = options;
        let power = baton_power::platform_power();
        if let Err(e) = power.restore_sleep() {
            log::warn!("恢复残留的睡眠设置失败：{e}");
        }
        let state_path = data_dir.join("state.toml");
        let local = LocalState::load(&state_path)
            .inspect_err(|e| log::warn!("读取本机状态失败，按未识别处理：{e}"))
            .unwrap_or_default();
        let host = hostname();
        let me = DeviceId(local.name(config.name.as_deref(), &host).to_owned());
        let store = Arc::new(PeerStore::open(&data_dir.join("peers.toml"))?);
        let known: Vec<PeerInfo> = store.list().iter().map(PairedPeer::info).collect();
        let identity = Arc::new(TlsIdentity::load_or_create(&data_dir.join("identity"))?);
        let learned = local.input_code;
        let learner = Arc::new(Mutex::new(InputCodeLearner::new(
            config.display.input_code,
            learned,
        )));
        let my_code = lock(&learner).current();

        let (events, events_rx) = mpsc::channel();
        baton_power::spawn_network_wake_watch(events.clone());
        let mut link_config = link_config(&me, &config, identity, store);
        link_config.me.input_code = my_code;
        link_config.observer = observer;
        let hotkey = local.hotkey(&config.hotkey).to_owned();
        let link = Arc::new(TcpPeerLink::start(link_config, events.clone())?);
        log::info!(
            "{} 监听 {}，热键 {hotkey}，数据目录 {}",
            me.0,
            link.local_addr(),
            data_dir.display()
        );
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
        let clipboard_sync = Arc::new(AtomicBool::new(local.clipboard_sync));
        let notifier: Arc<dyn Notifier> = Arc::from(notifier);
        let access: Arc<dyn ClipboardAccess> = Arc::new(SystemClipboard::new());
        let echoes = Arc::new(Mutex::new(EchoSet::new()));
        let clipboard = Arc::new(ClipboardSync::new(
            Arc::clone(&access),
            Arc::clone(&clipboard_sync),
            Arc::clone(&notifier),
            Arc::clone(&echoes),
        ));
        let bridge = Arc::new(ClipboardBridge::new(
            access,
            Arc::clone(&clipboard_sync),
            Arc::clone(&notifier),
            echoes,
            clamp_outbox_limit(local.clipboard_outbox_limit),
        ));
        let (stop_clipboard, clipboard_stopped) = mpsc::channel();
        let clipboard_loop = ClipboardLoop {
            bridge: Arc::clone(&bridge),
            link: Arc::clone(&link),
        };
        let clipboard_thread = thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || clipboard_loop.run(&clipboard_stopped))
            .map_err(|e| Error::Config(format!("无法启动剪贴板线程：{e}")))?;
        let files = FileService::start(
            Arc::clone(&link),
            baton_power::platform_power(),
            events.clone(),
            Arc::clone(&notifier),
            file_observer,
            FileSettings {
                auto_accept: local.file_auto_accept,
                receive_dir: local.file_receive_dir.as_ref().map(PathBuf::from),
            },
        );
        let ports = Ports {
            display: baton_display::platform_display(),
            power,
            input: baton_input::router_for(&config.input),
            wake: WakeChain::from_config(&config.power.wake_chain, baton_power::available_wakers()),
            link: Arc::clone(&link) as Arc<dyn baton_core::ports::PeerLink>,
            participants: vec![clipboard],
            notifier: Box::new(SharedNotifier(notifier)),
        };
        let display_mode = config.display.mode;
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
            display_mode,
            hotkey,
            events,
            link,
            learner,
            state_path,
            coordinator: Some(coordinator),
            stop_learning,
            learning,
            clipboard_sync,
            clipboard: bridge,
            stop_clipboard: Some(stop_clipboard),
            clipboard_thread: Some(clipboard_thread),
            files,
        })
    }

    pub fn files(&self) -> &Arc<FileService> {
        &self.files
    }

    pub fn display_mode(&self) -> DisplayMode {
        self.display_mode
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
        LocalState::update(&self.state_path, |state| state.input_code = None)?;
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

    pub fn set_hotkey_label(&mut self, hotkey: String) {
        self.hotkey = hotkey;
    }

    pub fn clipboard_sync(&self) -> bool {
        self.clipboard_sync.load(Ordering::SeqCst)
    }

    pub fn set_clipboard_sync(&self, on: bool) {
        self.clipboard_sync.store(on, Ordering::SeqCst);
    }

    /// 每个已配对对端发件箱里的条数。键是证书指纹，没有档案时是设备名。不含正文。
    pub fn clipboard_pending(&self) -> Vec<(String, usize)> {
        self.clipboard.pending_counts()
    }

    /// 本机名称立刻用于展示、mDNS 和之后的握手；不断开已有连接。
    pub fn set_name(&mut self, name: String) -> Result<()> {
        let id = DeviceId(name);
        let published = self.link.set_local_name(id.clone());
        self.me = id.clone();
        let _ = self.events.send(Event::LocalName(id));
        published
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
        self.files.shutdown();
        self.stop_learning = None;
        self.stop_clipboard = None;
        if let Some(learning) = self.learning.take()
            && learning.join().is_err()
        {
            log::warn!("输入源学习线程异常退出");
        }
        if let Some(clipboard) = self.clipboard_thread.take()
            && clipboard.join().is_err()
        {
            log::warn!("剪贴板线程异常退出");
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
const CLIPBOARD_POLL: Duration = Duration::from_millis(500);

struct ClipboardLoop {
    bridge: Arc<ClipboardBridge>,
    link: Arc<TcpPeerLink>,
}

impl ClipboardLoop {
    fn run(self, stopped: &Receiver<()>) {
        while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(CLIPBOARD_POLL) {
            let peers = clipboard_peers(&self.link);
            self.bridge.tick(&peers, |to, payload| {
                self.link
                    .send_data(to, ChannelId::CLIPBOARD, HandoffId::REALTIME, payload)
            });
        }
    }
}

fn clipboard_peers(link: &TcpPeerLink) -> Vec<PeerRef> {
    link.snapshot()
        .paired
        .into_iter()
        .map(|p| PeerRef {
            key: if p.peer.fingerprint.is_empty() {
                p.peer.name.clone()
            } else {
                p.peer.fingerprint.clone()
            },
            to: p.peer.device_id(),
            online: p.online,
        })
        .collect()
}

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
            if let Err(e) = LocalState::update(&self.state_path, |state| {
                state.input_code = Some(code);
            }) {
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

struct SharedNotifier(Arc<dyn Notifier>);

impl Notifier for SharedNotifier {
    fn notify(&self, message: &str) {
        self.0.notify(message);
    }
}
