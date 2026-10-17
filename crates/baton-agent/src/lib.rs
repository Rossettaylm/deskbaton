//! 组装各平台实现，启动链路与交棒协调器。无界面的 `deskbaton` 与面板 `deskbaton-gui` 共用。

pub mod hotkey;

use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use baton_core::config::Config;
use baton_core::coordinator::{Coordinator, Ports};
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::DeviceId;
use baton_core::ports::Notifier;
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
    coordinator: Option<JoinHandle<()>>,
}

impl Agent {
    /// 恢复上次残留的睡眠设置，加载身份与档案，开始监听、发现、拨号，并在独立线程运行协调器。
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

        let (events, events_rx) = mpsc::channel();
        let mut link_config = link_config(&me, &config, identity, store);
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
        let ports = Ports {
            display: baton_display::platform_display(),
            power,
            input: baton_input::router_for(&config.input),
            wake: WakeChain::from_config(&config.power.wake_chain, baton_power::available_wakers()),
            link: Arc::clone(&link) as Arc<dyn baton_core::ports::PeerLink>,
            participants: Vec::new(),
            notifier,
        };
        let coordinator = Coordinator::new(me.clone(), config, ports).with_known_peers(known);
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
            coordinator: Some(coordinator),
        })
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

fn link_config(
    me: &DeviceId,
    config: &Config,
    identity: Arc<TlsIdentity>,
    store: Arc<PeerStore>,
) -> LinkConfig {
    let info = PeerInfo {
        device_id: me.clone(),
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
