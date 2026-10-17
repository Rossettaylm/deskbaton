//! deskbaton：常驻 agent，组装各平台实现并驱动交棒协调器。

mod hotkey;

use std::io::BufRead;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::mpsc::{self, Sender};
use std::thread;

use baton_core::config::{Config, default_config_path};
use baton_core::coordinator::{Coordinator, Ports};
use baton_core::event::{Event, PeerInfo};
use baton_core::ids::DeviceId;
use baton_core::ports::Notifier;
use baton_core::wake::WakeChain;
use baton_core::{Error, Result};
use baton_net::{LinkConfig, PeerAddr, PeerStore, TcpPeerLink, TlsIdentity};
use clap::Parser;

#[derive(Parser)]
#[command(
    name = "deskbaton",
    version,
    about = "DeskBaton：一台显示器、多台电脑，一键交棒"
)]
struct Args {
    /// 配置文件路径，默认为平台配置目录下的 DeskBaton/config.toml
    #[arg(long)]
    config: Option<PathBuf>,
    /// 从标准输入触发交棒：每输入一行（回车）相当于按一次热键
    #[arg(long)]
    stdin: bool,
}

struct ConsoleNotifier;

impl Notifier for ConsoleNotifier {
    fn notify(&self, message: &str) {
        eprintln!("[DeskBaton] {message}");
    }
}

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("错误：{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<()> {
    let power = baton_power::platform_power();
    if let Err(e) = power.restore_sleep() {
        log::warn!("恢复残留的睡眠设置失败：{e}");
    }
    let path = match args.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    let config = load_config(&path)?;
    let me = DeviceId(config.name.clone().unwrap_or_else(hostname));
    let hotkey = hotkey::parse(&config.hotkey)?;

    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let (events_tx, events_rx) = mpsc::channel();
    let link = Arc::new(TcpPeerLink::start(
        link_config(&me, &config, &dir)?,
        events_tx.clone(),
    )?);
    log::info!(
        "{} 使用配置 {}，监听 {}，热键 {}",
        me.0,
        path.display(),
        link.local_addr(),
        config.hotkey
    );

    let ports = Ports {
        display: baton_display::platform_display(),
        power,
        input: baton_input::router_for(&config.input),
        wake: WakeChain::from_config(&config.power.wake_chain, baton_power::available_wakers()),
        link,
        participants: Vec::new(),
        notifier: Box::new(ConsoleNotifier),
    };
    let coordinator = Coordinator::new(me, config, ports);

    let shutdown = events_tx.clone();
    ctrlc::set_handler(move || {
        let _ = shutdown.send(Event::Shutdown);
    })
    .map_err(|e| Error::Config(format!("无法注册退出信号处理：{e}")))?;
    if args.stdin {
        spawn_stdin_trigger(events_tx.clone());
    }
    thread::Builder::new()
        .name("coordinator".into())
        .spawn(move || {
            coordinator.run(events_rx);
            log::info!("已退出");
            std::process::exit(0);
        })
        .map_err(|e| Error::Config(format!("无法启动协调器线程：{e}")))?;

    hotkey::run_main_loop(hotkey, events_tx)
}

fn link_config(me: &DeviceId, config: &Config, dir: &Path) -> Result<LinkConfig> {
    let info = PeerInfo {
        device_id: me.clone(),
        input_code: config.display.input_code,
        wol_mac: None,
        ip: None,
    };
    let listen = SocketAddr::from((Ipv4Addr::UNSPECIFIED, config.network.port));
    let identity = Arc::new(TlsIdentity::load_or_create(&dir.join("identity"))?);
    let store = Arc::new(PeerStore::open(&dir.join("peers.toml"))?);
    let mut link = LinkConfig::new(info, listen, identity, store);
    link.token.clone_from(&config.network.token);
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
    Ok(link)
}

fn load_config(path: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Error::Config(format!(
            "无法读取配置文件 {}：{e}。可参考仓库中的 config.example.toml",
            path.display()
        ))
    })?;
    Config::from_toml_str(&text)
}

fn hostname() -> String {
    let name = gethostname::gethostname().to_string_lossy().into_owned();
    name.trim_end_matches(".local").to_owned()
}

fn spawn_stdin_trigger(events: Sender<Event>) {
    thread::spawn(move || {
        eprintln!("[DeskBaton] 按回车交棒");
        for line in std::io::stdin().lock().lines() {
            if line.is_err() || events.send(Event::Hotkey).is_err() {
                break;
            }
        }
    });
}
