//! 面板进程内的 agent：热键、长连接与配对都在这里启动，退出时等协调器结束再退出进程。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use baton_agent::{Agent, Options, data_dir_of, hotkey, load_config};
use baton_core::Result;
use baton_core::config::default_config_path;
use baton_core::ports::Notifier;
use baton_net::{LinkObserver, TcpPeerLink};
use tauri::{AppHandle, Emitter, Manager};

use crate::{PANEL, show_panel};

thread_local! {
    /// 热键注册须留在主线程（Windows 上它持有本线程创建的窗口）。
    static HOTKEY: RefCell<Option<hotkey::Registration>> = const { RefCell::new(None) };
}

pub struct AgentSlot {
    agent: Mutex<Option<Agent>>,
    /// 与 `agent` 分开持有：链路回调观察者时不能再去拿 `agent` 的锁。
    pub link: Option<Arc<TcpPeerLink>>,
    pub name: Option<String>,
    pub hotkey: Option<String>,
    /// agent 启动失败的原因；面板照常可用。
    pub error: Option<String>,
    exiting: Arc<AtomicBool>,
}

impl AgentSlot {
    /// 在主线程调用。
    pub fn start(app: &AppHandle) -> Self {
        let exiting = Arc::new(AtomicBool::new(false));
        match try_start(app, &exiting) {
            Ok(agent) => Self {
                link: Some(Arc::clone(agent.link())),
                name: Some(agent.me().0.clone()),
                hotkey: Some(agent.hotkey().to_owned()),
                agent: Mutex::new(Some(agent)),
                error: None,
                exiting,
            },
            Err(e) => {
                log::error!("agent 启动失败：{e}");
                Self {
                    agent: Mutex::new(None),
                    link: None,
                    name: None,
                    hotkey: None,
                    error: Some(format!("交棒服务未启动：{e}")),
                    exiting,
                }
            }
        }
    }

    /// 进程退出前调用：等协调器结束，保证保持唤醒已释放。
    pub fn shutdown(&self) {
        self.exiting.store(true, Ordering::SeqCst);
        HOTKEY.with(|h| h.borrow_mut().take());
        let agent = self
            .agent
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(mut agent) = agent {
            agent.shutdown();
        }
    }
}

fn try_start(app: &AppHandle, exiting: &Arc<AtomicBool>) -> Result<Agent> {
    let path = default_config_path()?;
    let config = load_config(&path, true)?;
    let hotkey = hotkey::parse(&config.hotkey)?;
    let on_stopped = {
        let app = app.clone();
        let exiting = Arc::clone(exiting);
        Box::new(move || {
            if !exiting.load(Ordering::SeqCst) {
                app.exit(0);
            }
        })
    };
    let agent = Agent::start(Options {
        config,
        data_dir: data_dir_of(&path),
        notifier: Box::new(PanelNotifier(app.clone())),
        observer: Some(Arc::new(PanelObserver {
            app: app.clone(),
            shown_session: AtomicU64::new(0),
        })),
        on_stopped,
    })?;
    match hotkey::register(hotkey, agent.events()) {
        Ok(registration) => HOTKEY.with(|h| *h.borrow_mut() = Some(registration)),
        Err(e) => {
            log::warn!("{e}");
            notice(app, &e.to_string());
        }
    }
    Ok(agent)
}

fn notice(app: &AppHandle, message: &str) {
    let _ = app.emit_to(PANEL, "notice", message);
}

struct PanelNotifier(AppHandle);

impl Notifier for PanelNotifier {
    fn notify(&self, message: &str) {
        notice(&self.0, message);
    }
}

struct PanelObserver {
    app: AppHandle,
    /// 已为之弹出过面板的最新配对会话；会话编号递增。
    shown_session: AtomicU64,
}

impl LinkObserver for PanelObserver {
    fn changed(&self) {
        let _ = self.app.emit_to(PANEL, "devices-changed", ());
        let Some(slot) = self.app.try_state::<AgentSlot>() else {
            return;
        };
        let newest = slot.link.as_ref().and_then(|link| {
            link.snapshot()
                .sessions
                .iter()
                .filter(|s| s.awaiting_me)
                .map(|s| s.id)
                .max()
        });
        if let Some(id) = newest
            && self.shown_session.fetch_max(id, Ordering::SeqCst) < id
        {
            let app = self.app.clone();
            let _ = self.app.run_on_main_thread(move || show_panel(&app));
        }
    }

    fn notice(&self, message: &str) {
        notice(&self.app, message);
    }
}
