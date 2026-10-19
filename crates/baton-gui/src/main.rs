//! deskbaton-gui：菜单栏 / 系统托盘里的面板，进程内运行 agent（热键、长连接、配对）。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent;
mod bundle_config;
mod commands;
mod devices;
mod files;
mod notify;
mod placement;
mod prefs;
mod startup;
mod update;
mod view;

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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

#[derive(Default)]
struct LastHidden(Mutex<Option<Instant>>);

#[derive(Default)]
struct PendingOpenState {
    generation: u64,
    pending: Option<u64>,
}

#[derive(Default)]
pub(crate) struct PendingOpen(Mutex<PendingOpenState>);

impl PendingOpen {
    #[cfg(any(not(target_os = "macos"), test))]
    fn request(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.pending = Some(state.generation);
    }

    pub(crate) fn take(&self) -> Option<u64> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pending
            .take()
    }

    pub(crate) fn restore(&self, generation: u64) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if state.generation == generation && state.pending.is_none() {
            state.pending = Some(generation);
        }
    }

    fn clear(&self) {
        let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        state.generation = state.generation.wrapping_add(1);
        state.pending = None;
    }
}

/// 最近一次点击时托盘图标的位置，用来摆放面板。
#[derive(Default)]
struct TrayRect(Mutex<Option<Rect>>);

fn session_log_path(data_dir: &Path) -> PathBuf {
    data_dir.join("logs").join("deskbaton.log")
}

struct TeeLog {
    file: Mutex<File>,
}

impl Write for TeeLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(buf);
        self.file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let _ = io::stderr().flush();
        self.file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .flush()
    }
}

fn open_session_log() -> io::Result<(PathBuf, File)> {
    let dir = baton_core::config::data_dir().map_err(|e| io::Error::other(e.to_string()))?;
    let path = session_log_path(&dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(&path)?;
    Ok((path, file))
}

fn init_logger() {
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    let log_path = match open_session_log() {
        Ok((path, file)) => {
            builder.target(env_logger::Target::Pipe(Box::new(TeeLog {
                file: Mutex::new(file),
            })));
            Some(path)
        }
        Err(_) => None,
    };
    builder.init();
    if let Some(path) = log_path {
        log::info!("日志：{}", path.display());
    }
}

fn main() -> ExitCode {
    init_logger();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            match files::send_args(&argv) {
                Some(paths) => files::request_send(app, paths),
                None => show_panel(app),
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::Panel::load())
        .manage(files::FilesUi::default())
        .manage(LastHidden::default())
        .manage(PendingOpen::default())
        .manage(TrayRect::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_displays,
            commands::switch_input,
            commands::fit_panel,
            commands::host_platform,
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
            update::update_status,
            update::check_update,
            update::install_update,
            files::file_sessions,
            files::pick_and_send,
            files::accept_file,
            files::reject_file,
            files::cancel_file,
            files::dismiss_file,
            files::mark_files_seen,
            files::reveal_path,
            files::pending_send,
            files::send_pending,
            files::discard_pending,
            files::set_file_auto_accept,
            files::set_receive_dir,
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
            let install = startup::is_install_build();
            match startup::current_exe() {
                Ok(exe) => log::info!("安装版={install} 路径={}", exe.display()),
                Err(e) => log::info!("安装版={install} 路径未知：{e}"),
            }
            app.manage(update::UpdateSlot::new(install));
            app.manage(AgentSlot::start(app.handle()));
            #[cfg(target_os = "macos")]
            files::register_service(app.handle());
            let argv: Vec<String> = std::env::args().collect();
            if let Some(paths) = files::send_args(&argv) {
                files::request_send(app.handle(), paths);
            }
            let updater = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                crate::update::auto_check(updater).await;
            });
            let handle = app.handle().clone();
            if let Err(e) = ctrlc::set_handler(move || handle.exit(0)) {
                log::warn!("无法注册退出信号处理：{e}");
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == PANEL
                && matches!(event, WindowEvent::Focused(false))
                && !window
                    .state::<files::FilesUi>()
                    .picking
                    .load(std::sync::atomic::Ordering::SeqCst)
            {
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
    reveal_or_request(app, &window);
}

/// 有配对请求时主动弹出面板；已显示时不动。
pub(crate) fn show_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        return;
    }
    reveal_or_request(app, &window);
}

fn reveal_or_request(
    app: &AppHandle,
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))] window: &WebviewWindow,
) {
    #[cfg(target_os = "macos")]
    {
        place_panel(window);
        if let Err(e) = window.show().and_then(|()| window.set_focus()) {
            log::warn!("显示面板失败：{e}");
        }
        let _ = app.emit_to(PANEL, "panel-shown", ());
    }
    #[cfg(not(target_os = "macos"))]
    {
        app.state::<PendingOpen>().request();
        let _ = app.emit_to(PANEL, "panel-will-show", ());
    }
}

fn hide_panel(app: &AppHandle) {
    app.state::<PendingOpen>().clear();
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
        None => return Ok(()),
    };
    let panel = window.outer_size()?.to_logical::<f64>(window_scale);
    let (x, y) = placement::panel_origin(tray, (panel.width, panel.height), screen, GAP);
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

#[cfg(test)]
mod tests {
    use super::PendingOpen;

    #[test]
    fn pending_open_is_consumed_once() {
        let pending = PendingOpen::default();

        pending.request();

        assert!(pending.take().is_some());
        assert!(pending.take().is_none());
    }

    #[test]
    fn clearing_pending_open_cancels_request() {
        let pending = PendingOpen::default();

        pending.request();
        pending.clear();

        assert!(pending.take().is_none());
    }

    #[test]
    fn failed_open_can_restore_pending_request() {
        let pending = PendingOpen::default();
        pending.request();
        let generation = pending.take().unwrap();

        pending.restore(generation);

        assert_eq!(pending.take(), Some(generation));
    }

    #[test]
    fn clearing_pending_open_prevents_failed_request_restore() {
        let pending = PendingOpen::default();
        pending.request();
        let generation = pending.take().unwrap();

        pending.clear();
        pending.restore(generation);

        assert!(pending.take().is_none());
    }

    #[test]
    fn session_log_is_under_data_dir_logs() {
        let path = super::session_log_path(std::path::Path::new("/tmp/DeskBaton"));
        assert_eq!(
            path,
            std::path::Path::new("/tmp/DeskBaton/logs/deskbaton.log")
        );
        let main = include_str!("main.rs");
        assert!(
            main.contains("安装版={install}") && main.contains("exe.display()"),
            "启动必须写下安装版判定和可执行文件路径"
        );
    }
}
