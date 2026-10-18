//! deskbaton-gui：菜单栏 / 系统托盘里的面板，进程内运行 agent（热键、长连接、配对）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod bundle_config;
mod commands;
mod devices;
mod notify;
mod placement;
mod prefs;
mod startup;
mod view;

use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Rect, RunEvent, WebviewWindow, WindowEvent};

use crate::agent::AgentSlot;
use crate::placement::Bounds;

pub(crate) const PANEL: &str = "panel";
pub(crate) const PANEL_WIDTH: f64 = 360.0;

/// 点托盘图标时，面板会先因失焦被隐藏，紧接着收到点击；这段时间内的点击视为「关闭」而不是「重新打开」。
const BLUR_CLICK_GRACE: Duration = Duration::from_millis(300);
/// 面板与托盘图标、屏幕边缘的间距（逻辑像素）。
#[cfg(target_os = "macos")]
const GAP: f64 = 6.0;
#[cfg(not(target_os = "macos"))]
const GAP: f64 = 12.0;

#[cfg(target_os = "macos")]
const PANEL_ALIGN: placement::PanelAlign = placement::PanelAlign::CenterOnTray;
#[cfg(not(target_os = "macos"))]
const PANEL_ALIGN: placement::PanelAlign = placement::PanelAlign::WorkAreaEnd;

#[derive(Default)]
struct LastHidden(Mutex<Option<Instant>>);

/// 最近一次点击时托盘图标的位置，用来摆放面板。
#[derive(Default)]
struct TrayRect(Mutex<Option<Rect>>);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let app = tauri::Builder::default()
        .manage(commands::Panel::load())
        .manage(LastHidden::default())
        .manage(TrayRect::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_displays,
            commands::switch_input,
            commands::fit_panel,
            commands::quit,
            commands::app_version,
            devices::agent_status,
            devices::list_devices,
            devices::confirm_pairing,
            devices::pair_device,
            devices::unpair_device,
            devices::handoff_to,
            devices::relearn_input,
            prefs::prefs,
            prefs::set_name,
            prefs::set_hotkey,
            prefs::set_notify,
            prefs::set_clipboard_sync,
            prefs::set_autostart,
            prefs::suspend_hotkey,
            prefs::resume_hotkey,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            build_tray(app.handle())?;
            crate::prefs::apply_autostart_on_launch();
            #[cfg(target_os = "macos")]
            if crate::startup::running_bundled() {
                // SAFETY: 无参数。
                let rc = unsafe { baton_macos_sys::deskbaton_helper_register() };
                if rc != baton_macos_sys::OK {
                    log::warn!(
                        "未能登记睡眠助手，合盖切回时可能睡眠。请在系统设置 → 登录项与扩展里允许 DeskBaton"
                    );
                }
            }
            app.manage(AgentSlot::start(app.handle()));
            let handle = app.handle().clone();
            if let Err(e) = ctrlc::set_handler(move || handle.exit(0)) {
                log::warn!("无法注册退出信号处理：{e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == PANEL && matches!(event, WindowEvent::Focused(false)) {
                hide_panel(window.app_handle());
            }
        })
        .build(tauri::generate_context!());
    match app {
        Ok(app) => {
            app.run(|app, event| {
                if let RunEvent::Exit = event
                    && let Some(slot) = app.try_state::<AgentSlot>()
                {
                    slot.shutdown();
                }
            });
            ExitCode::SUCCESS
        }
        Err(e) => {
            log::error!("面板启动失败：{e}");
            ExitCode::FAILURE
        }
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit])?;
    TrayIconBuilder::with_id("main")
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("DeskBaton")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id() == "quit" {
                app.exit(0);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let app = tray.app_handle();
                *app.state::<TrayRect>()
                    .0
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(rect);
                toggle_panel(app);
            }
        })
        .build(app)?;
    Ok(())
}

fn toggle_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide_panel(app);
        return;
    }
    let just_hidden = app
        .state::<LastHidden>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some_and(|at| at.elapsed() < BLUR_CLICK_GRACE);
    if just_hidden {
        return;
    }
    place_panel(&window);
    if let Err(e) = window.show().and_then(|()| window.set_focus()) {
        log::warn!("显示面板失败：{e}");
    }
    let _ = app.emit_to(PANEL, "panel-shown", ());
}

/// 有配对请求时主动弹出面板；已显示时不动。
pub(crate) fn show_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        return;
    }
    place_panel(&window);
    if let Err(e) = window.show().and_then(|()| window.set_focus()) {
        log::warn!("显示面板失败：{e}");
    }
    let _ = app.emit_to(PANEL, "panel-shown", ());
}

fn hide_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        *app.state::<LastHidden>()
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }
}

pub(crate) fn place_panel(window: &WebviewWindow) {
    let live = window
        .app_handle()
        .tray_by_id("main")
        .and_then(|tray| tray.rect().ok().flatten());
    let stored = *window
        .state::<TrayRect>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Err(e) = try_place(window, live.or(stored)) {
        log::warn!("摆放面板失败：{e}");
    }
}

/// 托盘矩形和显示器位置都是「各屏逻辑坐标 × 该屏缩放比例」，不能跨屏直接比较；
/// 先按各屏比例换回逻辑坐标找出图标所在的屏幕，再在该屏的逻辑坐标里计算。
/// 对不上时退到主屏，没有主屏则退到第一块。
fn try_place(window: &WebviewWindow, rect: Option<Rect>) -> tauri::Result<()> {
    let monitors = window.available_monitors()?;
    if monitors.is_empty() {
        return Ok(());
    }
    let window_scale = window.scale_factor()?;
    let (screen, scale, tray) = match rect {
        Some(rect) => {
            let position = rect.position.to_physical::<f64>(window_scale);
            let size = rect.size.to_physical::<f64>(window_scale);
            let found = monitors.iter().find_map(|m| {
                let scale = m.scale_factor();
                let screen = logical_bounds(m);
                screen
                    .contains(position.x / scale, position.y / scale)
                    .then_some((screen, scale))
            });
            let (screen, scale) = match found {
                Some(hit) => hit,
                None => {
                    let screens: Vec<Bounds> = monitors.iter().map(logical_bounds).collect();
                    let screen = window
                        .primary_monitor()?
                        .map(|m| logical_bounds(&m))
                        .or_else(|| {
                            placement::screen_for_point(
                                position.x / window_scale,
                                position.y / window_scale,
                                &screens,
                            )
                        });
                    let Some(screen) = screen else {
                        return Ok(());
                    };
                    let scale = monitors
                        .iter()
                        .find(|m| logical_bounds(m) == screen)
                        .map_or(window_scale, tauri::Monitor::scale_factor);
                    (screen, scale)
                }
            };
            let tray = Bounds {
                x: position.x / scale,
                y: position.y / scale,
                width: size.width / scale,
                height: size.height / scale,
            };
            (screen, scale, tray)
        }
        None => {
            #[cfg(target_os = "macos")]
            return Ok(());
            #[cfg(not(target_os = "macos"))]
            {
                let primary = window.primary_monitor()?;
                let monitor = primary.as_ref().unwrap_or(&monitors[0]);
                let screen = logical_bounds(monitor);
                (
                    screen,
                    monitor.scale_factor(),
                    placement::work_area_end_tray(screen),
                )
            }
        }
    };
    let panel = window.outer_size()?.to_logical::<f64>(window_scale);
    let (x, y) =
        placement::panel_origin(tray, (panel.width, panel.height), screen, GAP, PANEL_ALIGN);
    // macOS 窗口坐标本身就是全局逻辑坐标；Windows 是全局物理坐标。
    #[cfg(target_os = "macos")]
    let result = {
        let _ = scale;
        window.set_position(tauri::LogicalPosition::new(x, y))
    };
    #[cfg(not(target_os = "macos"))]
    let result = window.set_position(tauri::PhysicalPosition::new(
        (x * scale).round() as i32,
        (y * scale).round() as i32,
    ));
    result
}

fn logical_bounds(monitor: &tauri::Monitor) -> Bounds {
    let scale = monitor.scale_factor();
    Bounds {
        x: f64::from(monitor.position().x) / scale,
        y: f64::from(monitor.position().y) / scale,
        width: f64::from(monitor.size().width) / scale,
        height: f64::from(monitor.size().height) / scale,
    }
}
