//! 面板「本机」卡片：名称、热键、开机自启、通知、剪贴板。只写 `state.toml`。

use std::path::PathBuf;
use std::sync::mpsc;

use baton_agent::{Agent, data_dir_of, hostname, hotkey, load_config};
use baton_core::autostart::{self, AutostartOnLaunch};
use baton_core::config::default_config_path;
use baton_core::state_file::LocalState;
use baton_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::agent::{self, AgentSlot};
use crate::startup;

#[derive(Serialize)]
pub struct PrefsView {
    /// 输入框内容；空表示用主机名。
    pub name: String,
    pub hostname: String,
    pub hotkey: String,
    pub notify: bool,
    pub clipboard_sync: bool,
    pub autostart: bool,
}

fn state_path() -> Result<PathBuf> {
    Ok(data_dir_of(&default_config_path()?).join("state.toml"))
}

fn load_local() -> Result<LocalState> {
    LocalState::load(&state_path()?)
}

pub fn apply_autostart_on_launch() {
    let seeded = load_local().map(|s| s.autostart_seeded).unwrap_or(false);
    let enabled = startup::is_enabled().unwrap_or(false);
    match autostart::on_launch(seeded, enabled) {
        AutostartOnLaunch::EnableAndSeed => {
            if let Err(e) = startup::set_enabled(true) {
                log::warn!("无法开启开机自启：{e}");
                return;
            }
            match state_path() {
                Ok(path) => {
                    if let Err(e) = LocalState::update(&path, |s| s.autostart_seeded = true) {
                        log::warn!("开机自启已开，但未能记下：{e}");
                    }
                }
                Err(e) => log::warn!("开机自启已开，但未能记下：{e}"),
            }
        }
        AutostartOnLaunch::RefreshPath => {
            if let Err(e) = startup::refresh_path() {
                log::warn!("无法更新开机自启路径：{e}");
            }
        }
        AutostartOnLaunch::LeaveOff => {}
    }
}

#[tauri::command]
pub fn prefs(slot: State<'_, AgentSlot>) -> std::result::Result<PrefsView, String> {
    read_prefs(&slot).map_err(|e| e.to_string())
}

fn read_prefs(slot: &AgentSlot) -> Result<PrefsView> {
    let config = load_config(&default_config_path()?, true).unwrap_or_default();
    let local = load_local().unwrap_or_default();
    let host = hostname();
    let name = match local.name.as_deref() {
        Some("") => String::new(),
        Some(name) => name.to_owned(),
        None => config.name.clone().unwrap_or_default(),
    };
    Ok(PrefsView {
        name,
        hostname: host,
        hotkey: slot
            .hotkey()
            .unwrap_or_else(|| local.hotkey(&config.hotkey).to_owned()),
        notify: slot.notify(),
        clipboard_sync: slot
            .with_agent(Agent::clipboard_sync)
            .unwrap_or(local.clipboard_sync),
        autostart: startup::is_enabled().unwrap_or(false),
    })
}

#[tauri::command]
pub fn set_name(slot: State<'_, AgentSlot>, name: String) -> std::result::Result<(), String> {
    let trimmed = name.trim().to_owned();
    LocalState::update(&state_path().map_err(|e| e.to_string())?, |s| {
        s.name = Some(trimmed.clone());
    })
    .map_err(|e| e.to_string())?;
    let host = hostname();
    let resolved = if trimmed.is_empty() { host } else { trimmed };
    slot.set_name(resolved).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_notify(slot: State<'_, AgentSlot>, notify: bool) -> std::result::Result<(), String> {
    LocalState::update(&state_path().map_err(|e| e.to_string())?, |s| {
        s.notify = notify;
    })
    .map_err(|e| e.to_string())?;
    slot.set_notify(notify);
    Ok(())
}

#[tauri::command]
pub fn set_clipboard_sync(
    slot: State<'_, AgentSlot>,
    clipboard_sync: bool,
) -> std::result::Result<(), String> {
    LocalState::update(&state_path().map_err(|e| e.to_string())?, |s| {
        s.clipboard_sync = clipboard_sync;
    })
    .map_err(|e| e.to_string())?;
    slot.set_clipboard_sync(clipboard_sync);
    Ok(())
}

#[tauri::command]
pub fn set_autostart(on: bool) -> std::result::Result<bool, String> {
    startup::set_enabled(on).map_err(|e| e.to_string())?;
    if on {
        let _ = LocalState::update(&state_path().map_err(|e| e.to_string())?, |s| {
            s.autostart_seeded = true;
        });
    }
    startup::is_enabled().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hotkey(app: AppHandle, hotkey: String) -> std::result::Result<String, String> {
    hotkey::parse(&hotkey).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    let text = hotkey.clone();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let slot = handle.state::<AgentSlot>();
        let result = apply_hotkey(&slot, &text);
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|_| "热键线程已关闭".to_owned())?
        .map_err(|e| e.to_string())?;
    Ok(hotkey)
}

#[tauri::command]
pub fn suspend_hotkey(app: AppHandle) -> std::result::Result<(), String> {
    let (tx, rx) = mpsc::channel();
    app.run_on_main_thread(move || {
        agent::drop_hotkey();
        let _ = tx.send(());
    })
    .map_err(|e| e.to_string())?;
    rx.recv().map_err(|_| "热键线程已关闭".to_owned())
}

#[tauri::command]
pub fn resume_hotkey(app: AppHandle) -> std::result::Result<(), String> {
    let (tx, rx) = mpsc::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let slot = handle.state::<AgentSlot>();
        let result = slot.resume_hotkey();
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|_| "热键线程已关闭".to_owned())?
        .map_err(|e| e.to_string())
}

fn apply_hotkey(slot: &AgentSlot, text: &str) -> Result<()> {
    let registration = slot.register_hotkey(text)?;
    LocalState::update(&state_path()?, |s| s.hotkey = Some(text.to_owned()))?;
    agent::take_hotkey(registration);
    slot.set_hotkey_label(text.to_owned());
    Ok(())
}

impl AgentSlot {
    fn hotkey(&self) -> Option<String> {
        self.with_agent(|a: &Agent| a.hotkey().to_owned())
    }

    fn set_name(&self, name: String) -> Result<()> {
        self.with_agent_mut(|a| a.set_name(name))
            .unwrap_or_else(|| Err(Error::Config("交棒服务未启动".into())))
    }

    fn set_hotkey_label(&self, hotkey: String) {
        self.with_agent_mut(|a| a.set_hotkey_label(hotkey));
    }
}
