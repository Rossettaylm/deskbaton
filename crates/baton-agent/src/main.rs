//! deskbaton：无界面的常驻 agent，供调试或不需要面板时使用。与面板 deskbaton-gui 不要同时运行。

use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc::Sender;
use std::thread;

use baton_agent::{Agent, Options, data_dir_of, hotkey, load_config};
use baton_core::config::default_config_path;
use baton_core::event::Event;
use baton_core::ports::Notifier;
use baton_core::{Error, Result};
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
    let path = match args.config {
        Some(path) => path,
        None => default_config_path()?,
    };
    let config = load_config(&path, false)?;
    let hotkey = hotkey::parse(&config.hotkey)?;
    log::info!("使用配置 {}", path.display());
    let agent = Agent::start(Options {
        config,
        data_dir: data_dir_of(&path),
        notifier: Box::new(ConsoleNotifier),
        observer: None,
        file_observer: None,
        on_stopped: Box::new(|| std::process::exit(0)),
    })?;

    let shutdown = agent.events();
    ctrlc::set_handler(move || {
        let _ = shutdown.send(Event::Shutdown);
    })
    .map_err(|e| Error::Config(format!("无法注册退出信号处理：{e}")))?;
    if args.stdin {
        spawn_stdin_trigger(agent.events());
    }
    hotkey::run_main_loop(hotkey, agent.events())
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
