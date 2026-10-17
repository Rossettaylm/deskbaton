//! 前端可调用的 Tauri 命令。DDC 调用会阻塞，统一放到阻塞线程池里执行。

use std::sync::{Arc, Mutex, PoisonError};

use baton_core::config::{Config, DisplayConfig, default_config_path};
use baton_core::ids::{DisplayId, InputCode};
use baton_core::ports::{DisplayControl, DisplayInfo};
use baton_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, LogicalSize, Manager, State};

use crate::agent::AgentSlot;
use crate::view::{self, DisplayView, Machines, PeerMachine};
use crate::{PANEL, PANEL_WIDTH, place_panel};

pub struct Panel {
    /// 同一时间只允许一个 I2C 事务。
    ddc: Mutex<Box<dyn DisplayControl>>,
    display: DisplayConfig,
    config_error: Option<String>,
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
        })
    }

    fn snapshot(&self, mut machines: Machines) -> Result<Snapshot> {
        machines.my_code = machines.my_code.or(self.display.input_code);
        let ddc = self.ddc.lock().unwrap_or_else(PoisonError::into_inner);
        let infos = ddc.list()?;
        let preferred = DisplayInfo::select(infos.clone(), &self.display.matcher)
            .ok()
            .map(|d| d.id);
        let displays = infos
            .iter()
            .map(|info| {
                let current = ddc.get_input(&info.id).ok();
                let screen = baton_display::screen_mode(&info.id)
                    .inspect_err(|e| log::info!("{} 的显示模式未知：{e}", info.id.0))
                    .ok();
                view::display_view(
                    info,
                    screen,
                    current,
                    &self.display.inputs,
                    preferred.as_ref() == Some(&info.id),
                    &machines,
                )
            })
            .collect();
        Ok(Snapshot {
            displays,
            config_error: self.config_error.clone(),
        })
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
    blocking(move || panel.snapshot(machines)).await
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
    Ok(())
}

#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}
