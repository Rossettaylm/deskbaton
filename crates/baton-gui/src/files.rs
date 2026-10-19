//! 文件快传的面板命令：右键发送、选择框、会话卡片、接收设置、系统「发送到」/ 服务菜单。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use baton_agent::data_dir_of;
use baton_core::config::default_config_path;
use baton_core::files::{SessionId, SessionView};
use baton_core::ids::{DeviceId, Platform};
use baton_core::state_file::LocalState;
use baton_core::{Error, Result};
use baton_files::FileService;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::agent::{AgentSlot, focus_files};
use crate::{PANEL, notify};

/// 面板命令行参数：其后的参数都是要发送的路径。
pub const SEND_FLAG: &str = "--send";

#[derive(Default)]
pub struct FilesUi {
    /// 系统选择框打开期间，面板失焦不收起。
    pub picking: AtomicBool,
    /// 从系统菜单发起、等用户选目标的路径。
    pending: Mutex<Vec<PathBuf>>,
    /// 上次发送的设备，选择器里排第一并默认选中。
    last_target: Mutex<Option<String>>,
}

impl FilesUi {
    fn last_target(&self) -> Option<String> {
        self.last_target
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn remember(&self, peer: &str) {
        *self
            .last_target
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(peer.to_owned());
    }

    fn take_pending(&self) -> Vec<PathBuf> {
        std::mem::take(&mut *self.pending.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// `argv` 里 [`SEND_FLAG`] 之后的路径；没有该参数时为 `None`。
pub fn send_args(argv: &[String]) -> Option<Vec<PathBuf>> {
    let at = argv.iter().position(|a| a == SEND_FLAG)?;
    Some(argv[at + 1..].iter().map(PathBuf::from).collect())
}

fn service(slot: &AgentSlot) -> std::result::Result<&Arc<FileService>, String> {
    slot.files
        .as_ref()
        .ok_or_else(|| slot.error.clone().unwrap_or_else(|| "未能启动".to_owned()))
}

fn state_path() -> Result<PathBuf> {
    Ok(data_dir_of(&default_config_path()?).join("state.toml"))
}

fn text(e: impl ToString) -> String {
    e.to_string()
}

/// 后台发送；`walk` 大文件夹可能较慢，不占命令线程。
fn spawn_send(app: &AppHandle, peer: String, paths: Vec<PathBuf>) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("file-send".into())
        .spawn(move || {
            let slot = app.state::<AgentSlot>();
            let Ok(files) = service(&slot) else {
                return;
            };
            app.state::<FilesUi>().remember(&peer);
            if let Err(e) = files.send(&DeviceId(peer), &paths) {
                let _ = app.emit_to(PANEL, "notice", e.to_string());
                if slot.notify() {
                    notify::show(&e.to_string());
                }
            }
        });
}

/// 系统「发送到」或服务菜单交来的路径：只有一台已配对设备时直接发，多台时弹面板选择。
pub fn request_send(app: &AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    let slot = app.state::<AgentSlot>();
    let Some(link) = slot.link.as_ref() else {
        notify::show("DeskBaton 未能启动，无法发送");
        return;
    };
    let paired = link.snapshot().paired;
    match paired.as_slice() {
        [] => {
            notify::show("还没有配对的设备");
            let _ = app.emit_to(PANEL, "notice", "还没有配对的设备");
        }
        [only] => {
            spawn_send(app, only.peer.name.clone(), paths);
            focus_files(app);
        }
        _ => {
            *app.state::<FilesUi>()
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = paths;
            let _ = app.emit_to(PANEL, "send-pick", ());
            focus_files(app);
        }
    }
}

#[derive(Serialize)]
pub struct PendingSend {
    count: usize,
    names: Vec<String>,
    targets: Vec<Target>,
}

#[derive(Serialize)]
struct Target {
    name: String,
    platform: Option<Platform>,
    online: bool,
}

#[tauri::command]
pub fn pending_send(slot: State<'_, AgentSlot>, ui: State<'_, FilesUi>) -> Option<PendingSend> {
    let pending = ui.pending.lock().unwrap_or_else(PoisonError::into_inner);
    if pending.is_empty() {
        return None;
    }
    let mut targets: Vec<Target> = slot
        .link
        .as_ref()?
        .snapshot()
        .paired
        .into_iter()
        .map(|p| Target {
            name: p.peer.name,
            platform: p.peer.platform,
            online: p.online,
        })
        .collect();
    if let Some(last) = ui.last_target() {
        targets.sort_by_key(|t| t.name != last);
    }
    Some(PendingSend {
        count: pending.len(),
        names: pending
            .iter()
            .take(3)
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect(),
        targets,
    })
}

#[tauri::command]
pub fn send_pending(app: AppHandle, ui: State<'_, FilesUi>, peer: String) {
    let paths = ui.take_pending();
    spawn_send(&app, peer, paths);
}

#[tauri::command]
pub fn discard_pending(ui: State<'_, FilesUi>) {
    ui.take_pending();
}

/// 右键「发送文件… / 发送文件夹…」：弹系统选择框，选好后发给 `peer`。返回是否选了内容。
#[tauri::command]
pub async fn pick_and_send(
    app: AppHandle,
    peer: String,
    folder: bool,
) -> std::result::Result<bool, String> {
    let picker = app.clone();
    let paths = tauri::async_runtime::spawn_blocking(move || pick(&picker, folder))
        .await
        .map_err(text)?;
    if paths.is_empty() {
        return Ok(false);
    }
    spawn_send(&app, peer, paths);
    Ok(true)
}

fn pick(app: &AppHandle, folder: bool) -> Vec<PathBuf> {
    let ui = app.state::<FilesUi>();
    ui.picking.store(true, Ordering::SeqCst);
    let dialog = app.dialog().file();
    let picked = if folder {
        dialog
            .set_title("选择要发送的文件夹")
            .blocking_pick_folders()
    } else {
        dialog.set_title("选择要发送的文件").blocking_pick_files()
    };
    ui.picking.store(false, Ordering::SeqCst);
    if let Some(window) = app.get_webview_window(PANEL) {
        let _ = window.set_focus();
    }
    picked
        .unwrap_or_default()
        .into_iter()
        .filter_map(|p| p.into_path().ok())
        .collect()
}

#[tauri::command]
pub fn file_sessions(slot: State<'_, AgentSlot>) -> std::result::Result<Vec<SessionView>, String> {
    Ok(service(&slot)?.snapshot())
}

#[tauri::command]
pub fn accept_file(
    slot: State<'_, AgentSlot>,
    peer: String,
    id: SessionId,
) -> std::result::Result<(), String> {
    service(&slot)?.accept(&DeviceId(peer), id).map_err(text)
}

#[tauri::command]
pub fn reject_file(
    slot: State<'_, AgentSlot>,
    peer: String,
    id: SessionId,
) -> std::result::Result<(), String> {
    service(&slot)?.reject(&DeviceId(peer), id);
    Ok(())
}

#[tauri::command]
pub fn cancel_file(
    slot: State<'_, AgentSlot>,
    peer: String,
    id: SessionId,
) -> std::result::Result<(), String> {
    service(&slot)?.cancel(&DeviceId(peer), id);
    Ok(())
}

#[tauri::command]
pub fn dismiss_file(
    slot: State<'_, AgentSlot>,
    peer: String,
    id: SessionId,
) -> std::result::Result<(), String> {
    service(&slot)?.dismiss(&DeviceId(peer), id);
    Ok(())
}

/// 面板已经展示过接收结果，接棒时不再为它们弹出。
#[tauri::command]
pub fn mark_files_seen(slot: State<'_, AgentSlot>) -> std::result::Result<(), String> {
    service(&slot)?.mark_seen();
    Ok(())
}

#[tauri::command]
pub fn reveal_path(path: String) {
    reveal(Path::new(&path));
}

/// 在访达 / 资源管理器中选中 `path`。
pub fn reveal(path: &Path) {
    let result = {
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn()
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            std::process::Command::new("explorer.exe")
                .raw_arg(format!("/select,\"{}\"", path.display()))
                .spawn()
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            std::process::Command::new("xdg-open")
                .arg(path.parent().unwrap_or(path))
                .spawn()
        }
    };
    if let Err(e) = result {
        log::warn!("无法打开 {}：{e}", path.display());
    }
}

#[derive(Serialize)]
pub struct ReceivePrefs {
    pub auto_accept: bool,
    /// 实际使用的接收位置。
    pub dir: String,
    /// 用户改过接收位置。
    pub custom: bool,
}

pub fn receive_prefs(slot: &AgentSlot) -> ReceivePrefs {
    let local = state_path()
        .and_then(|p| LocalState::load(&p))
        .unwrap_or_default();
    let dir = slot
        .files
        .as_ref()
        .and_then(|f| f.receive_dir().ok())
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    ReceivePrefs {
        auto_accept: slot
            .files
            .as_ref()
            .map_or(local.file_auto_accept, |f| f.auto_accept()),
        dir,
        custom: local.file_receive_dir.is_some(),
    }
}

#[tauri::command]
pub fn set_file_auto_accept(
    slot: State<'_, AgentSlot>,
    on: bool,
) -> std::result::Result<(), String> {
    LocalState::update(&state_path().map_err(text)?, |s| s.file_auto_accept = on).map_err(text)?;
    if let Some(files) = &slot.files {
        files.set_auto_accept(on);
    }
    Ok(())
}

/// 选新的接收位置；`reset` 时恢复为系统「下载」文件夹。返回实际位置，取消选择时为空。
#[tauri::command]
pub async fn set_receive_dir(
    app: AppHandle,
    reset: bool,
) -> std::result::Result<Option<String>, String> {
    let dir = if reset {
        None
    } else {
        let picker = app.clone();
        let picked = tauri::async_runtime::spawn_blocking(move || {
            let ui = picker.state::<FilesUi>();
            ui.picking.store(true, Ordering::SeqCst);
            let dir = picker
                .dialog()
                .file()
                .set_title("选择接收位置")
                .blocking_pick_folder();
            ui.picking.store(false, Ordering::SeqCst);
            if let Some(window) = picker.get_webview_window(PANEL) {
                let _ = window.set_focus();
            }
            dir
        })
        .await
        .map_err(text)?;
        match picked.map(|p| p.into_path()) {
            Some(Ok(dir)) => Some(dir),
            Some(Err(e)) => return Err(e.to_string()),
            None => return Ok(None),
        }
    };
    let stored = dir.as_ref().map(|d| d.display().to_string());
    LocalState::update(&state_path().map_err(text)?, |s| {
        s.file_receive_dir.clone_from(&stored);
    })
    .map_err(text)?;
    let slot = app.state::<AgentSlot>();
    let files = service(&slot)?;
    files.set_receive_dir(dir);
    files
        .receive_dir()
        .map(|d| Some(d.display().to_string()))
        .map_err(|e: Error| e.to_string())
}

/// 服务菜单「用 DeskBaton 发送」：原生回调把路径转给 [`request_send`]。
#[cfg(target_os = "macos")]
pub fn register_service(app: &AppHandle) {
    use std::ffi::{CStr, c_char};
    use std::sync::OnceLock;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    unsafe extern "C" fn on_send(paths: *const *const c_char, count: u32) {
        let Some(app) = APP.get() else {
            return;
        };
        let paths: Vec<PathBuf> = (0..count as usize)
            .filter_map(|i| {
                // SAFETY: 原生侧保证 `paths` 指向 `count` 个在回调期间有效的 C 字符串。
                let raw = unsafe { *paths.add(i) };
                if raw.is_null() {
                    return None;
                }
                // SAFETY: 同上，`raw` 以 NUL 结尾。
                let text = unsafe { CStr::from_ptr(raw) };
                Some(PathBuf::from(text.to_string_lossy().into_owned()))
            })
            .collect();
        request_send(app, paths);
    }

    let _ = APP.set(app.clone());
    // SAFETY: 在主线程调用，回调为本模块的 `extern "C"` 函数。
    let rc = unsafe { baton_macos_sys::deskbaton_register_send_service(Some(on_send)) };
    if rc != baton_macos_sys::OK {
        log::warn!("未能登记服务菜单「用 DeskBaton 发送」");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_args_take_everything_after_the_flag() {
        let argv: Vec<String> = ["deskbaton-gui.exe", "--send", r"C:\a b.txt", r"D:\照片"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(
            send_args(&argv),
            Some(vec![
                PathBuf::from(r"C:\a b.txt"),
                PathBuf::from(r"D:\照片")
            ])
        );
        assert_eq!(send_args(&argv[..1]), None);
        assert_eq!(send_args(&argv[..2]), Some(Vec::new()));
    }
}
