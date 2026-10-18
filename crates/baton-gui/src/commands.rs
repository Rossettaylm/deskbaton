//! 前端可调用的 Tauri 命令。DDC 调用会阻塞，统一放到阻塞线程池里执行。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use baton_core::config::{Config, DisplayConfig, default_config_path};
use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use baton_display::ScreenMode;
use serde::Serialize;
use tauri::{AppHandle, Emitter, LogicalSize, Manager, State};

use crate::agent::AgentSlot;
use crate::view::{self, DisplayView, Machines, PeerMachine};
use crate::{PANEL, PANEL_WIDTH, PendingOpen, place_panel};

/// 接棒后 CoreGraphics 往往晚于 IOKit，过一会儿再读一次屏幕模式。
const SCREEN_RETRY_AFTER: Duration = Duration::from_millis(800);

pub struct Panel {
    /// 同一时间只允许一个 I2C 事务。
    ddc: Mutex<Box<dyn DisplayControl>>,
    display: DisplayConfig,
    config_error: Option<String>,
    screens: Mutex<HashMap<String, ScreenMode>>,
    retrying: AtomicBool,
}

impl Panel {
    pub fn load() -> Arc<Self> {
        let (display, config_error) = match load_display_config() {
            Ok(display) => (display, None),
            Err(e) => {
                log::warn!("{e}");
                (DisplayConfig::default(), Some(e.to_string()))
            }
        };
        Arc::new(Self {
            ddc: Mutex::new(baton_display::platform_display()),
            display,
            config_error,
            screens: Mutex::new(HashMap::new()),
            retrying: AtomicBool::new(false),
        })
    }

    fn snapshot(&self, mut machines: Machines) -> Result<(Snapshot, bool)> {
        machines.my_code = machines.my_code.or(self.display.input_code);
        let ddc = self.ddc.lock().unwrap_or_else(PoisonError::into_inner);
        let infos = ddc.list()?;
        let preferred = DisplayInfo::select(infos.clone(), &self.display.matcher)
            .ok()
            .map(|d| d.id);
        let mut screens = self.screens.lock().unwrap_or_else(PoisonError::into_inner);
        let mut retry = false;
        let mut displays = Vec::with_capacity(infos.len());
        for info in &infos {
            let current = ddc.get_input(&info.id).ok();
            let fresh = match baton_display::screen_mode(&info.id) {
                Ok(mode) => Some(mode),
                Err(e) => {
                    if !screens.contains_key(&info.id.0) {
                        log::info!("{} 的显示模式未知：{e}", info.id.0);
                    }
                    retry = true;
                    None
                }
            };
            let screen = view::cached_mode(&mut screens, &info.id.0, fresh);
            displays.push(view::display_view(
                info,
                screen,
                current,
                &self.display.inputs,
                preferred.as_ref() == Some(&info.id),
                &machines,
            ));
        }
        Ok((
            Snapshot {
                displays,
                config_error: self.config_error.clone(),
            },
            retry,
        ))
    }

    fn switch(&self, id: &DisplayId, code: InputCode) -> Result<SwitchOutcome> {
        let ddc = self.ddc.lock().unwrap_or_else(PoisonError::into_inner);
        ddc.set_input(id, code)?;
        Ok(match ddc.get_input(id) {
            Ok(now) if now == code => SwitchOutcome::Confirmed {
                code: now.0,
                label: view::input_label(&self.display.inputs, now),
            },
            _ => SwitchOutcome::LeftLocal {
                label: view::input_label(&self.display.inputs, code),
            },
        })
    }
}

/// 文件不存在时用默认配置；存在但解析失败时报错，由调用方退回默认配置。
fn load_display_config() -> Result<DisplayConfig> {
    let path = default_config_path()?;
    match std::fs::read_to_string(&path) {
        Ok(text) => Config::from_toml_str(&text)
            .map(|c| c.display)
            .map_err(|e| Error::Config(format!("配置文件 {} 有误：{e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DisplayConfig::default()),
        Err(e) => Err(Error::Config(format!(
            "无法读取配置文件 {}：{e}",
            path.display()
        ))),
    }
}

#[derive(Serialize)]
pub struct Snapshot {
    displays: Vec<DisplayView>,
    config_error: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SwitchOutcome {
    /// 切换后回读到目标编码。
    Confirmed { code: u16, label: String },
    /// 写入成功但回读失败或不是目标编码：本机已不再持有画面，不算失败。
    LeftLocal { label: String },
}

async fn blocking<T: Send + 'static>(
    op: impl FnOnce() -> Result<T> + Send + 'static,
) -> std::result::Result<T, String> {
    tauri::async_runtime::spawn_blocking(op)
        .await
        .map_err(|e| format!("后台任务异常：{e}"))?
        .map_err(|e| e.to_string())
}

/// 本机名称与已配对设备的输入口；交棒服务未启动时为空。
fn machines(app: &AppHandle) -> Machines {
    let Some(slot) = app.try_state::<AgentSlot>() else {
        return Machines::default();
    };
    let peers = slot.link.as_ref().map_or_else(Vec::new, |link| {
        link.snapshot()
            .paired
            .into_iter()
            .map(|status| PeerMachine {
                name: status.peer.name,
                platform: status.peer.platform,
                code: status.peer.input_code,
                online: status.online,
                holding: status.holding,
            })
            .collect()
    });
    Machines {
        me: slot
            .with_agent(|a| a.me().0.clone())
            .or_else(|| slot.name.clone()),
        my_code: slot.local_input_code(),
        peers,
    }
}

#[tauri::command]
pub async fn list_displays(
    app: AppHandle,
    panel: State<'_, Arc<Panel>>,
) -> std::result::Result<Snapshot, String> {
    let panel = Arc::clone(&panel);
    let machines = machines(&app);
    let retry_panel = Arc::clone(&panel);
    let (snapshot, retry) = blocking(move || panel.snapshot(machines)).await?;
    if retry {
        schedule_screen_retry(app, retry_panel);
    }
    Ok(snapshot)
}

fn schedule_screen_retry(app: AppHandle, panel: Arc<Panel>) {
    if panel.retrying.swap(true, Ordering::SeqCst) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("deskbaton-screen-retry".into())
        .spawn(move || {
            std::thread::sleep(SCREEN_RETRY_AFTER);
            panel.retrying.store(false, Ordering::SeqCst);
            let _ = app.emit_to(PANEL, "devices-changed", ());
        });
}

#[tauri::command]
pub async fn switch_input(
    panel: State<'_, Arc<Panel>>,
    id: String,
    code: u16,
) -> std::result::Result<SwitchOutcome, String> {
    let panel = Arc::clone(&panel);
    blocking(move || panel.switch(&DisplayId(id), InputCode(code))).await
}

/// 前端渲染完成后按内容高度调整窗口，并重新贴到托盘图标旁。
#[tauri::command]
pub fn fit_panel(app: AppHandle, height: f64) -> std::result::Result<(), String> {
    let window = app.get_webview_window(PANEL).ok_or("面板窗口不存在")?;
    window
        .set_size(LogicalSize::new(PANEL_WIDTH, height.ceil()))
        .map_err(|e| e.to_string())?;
    place_panel(&window);
    if app.state::<PendingOpen>().take() && !window.is_visible().unwrap_or(false) {
        if let Err(e) = window.show().and_then(|()| window.set_focus()) {
            return Err(e.to_string());
        }
        let _ = app.emit_to(PANEL, "panel-shown", ());
    }
    Ok(())
}

#[tauri::command]
pub fn host_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "windows"
    }
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
