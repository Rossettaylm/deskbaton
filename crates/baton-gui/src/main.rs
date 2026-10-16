//! deskbaton-gui：菜单栏 / 系统托盘里的显示器面板。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod placement;
mod view;

use std::process::ExitCode;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Rect, WebviewWindow, WindowEvent};

use crate::placement::Bounds;

pub(crate) const PANEL: &str = "panel";
pub(crate) const PANEL_WIDTH: f64 = 360.0;

/// 点托盘图标时，面板会先因失焦被隐藏，紧接着收到点击；这段时间内的点击视为「关闭」而不是「重新打开」。
const BLUR_CLICK_GRACE: Duration = Duration::from_millis(300);
/// 面板与托盘图标、屏幕边缘的间距（逻辑像素）。
const GAP: f64 = 6.0;

#[derive(Default)]
struct LastHidden(Mutex<Option<Instant>>);

/// 最近一次点击时托盘图标的位置，用来摆放面板。
#[derive(Default)]
struct TrayRect(Mutex<Option<Rect>>);

fn main() -> ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let result = tauri::Builder::default()
        .manage(commands::Panel::load())
        .manage(LastHidden::default())
        .manage(TrayRect::default())
        .invoke_handler(tauri::generate_handler![
            commands::list_displays,
            commands::switch_input,
            commands::fit_panel,
            commands::quit,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == PANEL && matches!(event, WindowEvent::Focused(false)) {
                hide_panel(window.app_handle());
            }
        })
        .run(tauri::generate_context!());
    match result {
        Ok(()) => ExitCode::SUCCESS,
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
        .tooltip("DeskBaton 显示器面板")
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
    let rect = *window
        .state::<TrayRect>()
        .0
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let Some(rect) = rect else {
        return;
    };
    if let Err(e) = try_place(window, rect) {
        log::warn!("摆放面板失败：{e}");
    }
}

fn try_place(window: &WebviewWindow, rect: Rect) -> tauri::Result<()> {
    let scale = window.scale_factor()?;
    let position = rect.position.to_physical::<f64>(scale);
    let size = rect.size.to_physical::<f64>(scale);
    let Some(monitor) = window.monitor_from_point(position.x, position.y)? else {
        return Ok(());
    };
    let tray = Bounds {
        x: position.x,
        y: position.y,
        width: size.width,
        height: size.height,
    };
    let screen = Bounds {
        x: f64::from(monitor.position().x),
        y: f64::from(monitor.position().y),
        width: f64::from(monitor.size().width),
        height: f64::from(monitor.size().height),
    };
    let panel = window.outer_size()?;
    let (x, y) = placement::panel_origin(
        tray,
        (f64::from(panel.width), f64::from(panel.height)),
        screen,
        GAP * monitor.scale_factor(),
    );
    window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32))
}
