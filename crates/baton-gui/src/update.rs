//! 安装版程序内更新：状态槽、检查与安装。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use baton_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::PANEL;
use crate::agent::AgentSlot;
use crate::notify;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    Idle,
    Available,
    Installing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateStatus {
    pub supported: bool,
    pub kind: UpdateKind,
    pub version: Option<String>,
}

struct Inner {
    supported: bool,
    kind: UpdateKind,
    version: Option<String>,
    announced: Option<String>,
}

pub struct UpdateSlot {
    inner: Mutex<Inner>,
    fetching: AtomicBool,
}

impl UpdateSlot {
    pub fn new(supported: bool) -> Self {
        Self {
            inner: Mutex::new(Inner {
                supported,
                kind: UpdateKind::Idle,
                version: None,
                announced: None,
            }),
            fetching: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn status(&self) -> UpdateStatus {
        let inner = self.lock();
        UpdateStatus {
            supported: inner.supported,
            kind: inner.kind,
            version: inner.version.clone(),
        }
    }

    fn apply_remote(&self, remote: Option<String>) -> UpdateStatus {
        let mut inner = self.lock();
        if inner.kind == UpdateKind::Installing {
            return UpdateStatus {
                supported: inner.supported,
                kind: inner.kind,
                version: inner.version.clone(),
            };
        }
        match remote {
            Some(version) => {
                inner.kind = UpdateKind::Available;
                inner.version = Some(version);
            }
            None => {
                inner.kind = UpdateKind::Idle;
                inner.version = None;
            }
        }
        UpdateStatus {
            supported: inner.supported,
            kind: inner.kind,
            version: inner.version.clone(),
        }
    }

    pub fn take_announce(&self) -> Option<String> {
        let mut inner = self.lock();
        let version = inner.version.clone()?;
        if inner.kind != UpdateKind::Available {
            return None;
        }
        if inner.announced.as_deref() == Some(version.as_str()) {
            return None;
        }
        inner.announced = Some(version.clone());
        Some(version)
    }

    pub fn begin_install(&self) -> Result<String> {
        let mut inner = self.lock();
        match inner.kind {
            UpdateKind::Installing => Err(Error::Config("正在升级…".into())),
            UpdateKind::Available => {
                let version = inner
                    .version
                    .clone()
                    .ok_or_else(|| Error::Config("没有可用更新".into()))?;
                inner.kind = UpdateKind::Installing;
                Ok(version)
            }
            UpdateKind::Idle => Err(Error::Config("没有可用更新".into())),
        }
    }

    pub fn abort_install(&self) {
        let mut inner = self.lock();
        if inner.kind == UpdateKind::Installing && inner.version.is_some() {
            inner.kind = UpdateKind::Available;
        }
    }
}

#[cfg(test)]
fn check_with(
    slot: &UpdateSlot,
    fetch: impl FnOnce() -> Result<Option<String>>,
) -> Result<UpdateStatus> {
    let status = slot.status();
    if !status.supported || status.kind == UpdateKind::Installing {
        return Ok(status);
    }
    let remote = fetch()?;
    Ok(slot.apply_remote(remote))
}

pub fn progress_text(loaded: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if total > 0 => {
            format!(
                "正在下载更新 {} / {} MB",
                loaded / (1024 * 1024),
                total / (1024 * 1024)
            )
        }
        _ => "正在下载更新…（升级会短暂退出）".into(),
    }
}

pub fn found_notice(version: &str) -> String {
    format!("发现新版本 v{version}")
}

fn cmd_err(e: Error) -> String {
    match e {
        Error::Config(msg) => msg,
        other => other.to_string(),
    }
}

fn updater_user_text(prefix: &str, text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("sign") {
        "更新包校验失败".into()
    } else if lower.contains("fallback platform")
        || lower.contains("not found in the response `platforms`")
    {
        format!("{prefix}当前系统没有对应的更新包")
    } else {
        format!("{prefix}{text}")
    }
}

fn updater_err(prefix: &str, e: tauri_plugin_updater::Error) -> Error {
    Error::Config(updater_user_text(prefix, &e.to_string()))
}

async fn fetch_remote(app: &AppHandle) -> Result<Option<String>> {
    let updater = app
        .updater()
        .map_err(|e| updater_err("无法检查更新：", e))?;
    match updater.check().await {
        Ok(update) => Ok(update.map(|u| u.version)),
        Err(e) => Err(updater_err("无法检查更新：", e)),
    }
}

async fn check_remote(app: &AppHandle) -> Result<UpdateStatus> {
    let slot = app.state::<UpdateSlot>();
    let status = slot.status();
    if !status.supported || status.kind == UpdateKind::Installing {
        return Ok(status);
    }
    if slot.fetching.swap(true, Ordering::SeqCst) {
        return Ok(status);
    }
    let remote = fetch_remote(app).await;
    slot.fetching.store(false, Ordering::SeqCst);
    Ok(slot.apply_remote(remote?))
}

fn announce(app: &AppHandle, system_notify: bool) {
    let slot = app.state::<UpdateSlot>();
    let Some(version) = slot.take_announce() else {
        return;
    };
    let message = found_notice(&version);
    let _ = app.emit_to(PANEL, "notice", &message);
    let _ = app.emit_to(PANEL, "update-available", &version);
    if system_notify {
        notify::show(&message);
    }
}

pub async fn auto_check(app: AppHandle) {
    match check_remote(&app).await {
        Ok(_) => {
            let system_notify = app
                .try_state::<AgentSlot>()
                .is_some_and(|slot| slot.notify());
            announce(&app, system_notify);
        }
        Err(e) => log::info!("{e}"),
    }
}

#[derive(Clone, Copy, Serialize)]
struct UpdateProgress {
    loaded: u64,
    total: Option<u64>,
}

#[tauri::command]
pub fn update_status(slot: State<'_, UpdateSlot>) -> UpdateStatus {
    slot.status()
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> std::result::Result<UpdateStatus, String> {
    match check_remote(&app).await {
        Ok(status) => {
            if status.supported && status.kind == UpdateKind::Idle {
                let _ = app.emit_to(PANEL, "notice", "已是最新版本");
            }
            announce(&app, false);
            Ok(status)
        }
        Err(e) => Err(cmd_err(e)),
    }
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> std::result::Result<(), String> {
    do_install(app).await.map_err(cmd_err)
}

async fn do_install(app: AppHandle) -> Result<()> {
    let slot = app.state::<UpdateSlot>();
    let _version = slot.begin_install()?;
    let _ = app.emit_to(PANEL, "notice", progress_text(0, None));
    let updater = app
        .updater()
        .map_err(|e| updater_err("无法安装更新：", e))?;
    let Some(update) = updater
        .check()
        .await
        .map_err(|e| updater_err("无法检查更新：", e))?
    else {
        slot.abort_install();
        slot.apply_remote(None);
        return Err(Error::Config("没有可用更新".into()));
    };
    let loaded = AtomicU64::new(0);
    let progress_app = app.clone();
    let result = update
        .download_and_install(
            |chunk, total| {
                let now = loaded.fetch_add(chunk as u64, Ordering::Relaxed) + chunk as u64;
                let _ = progress_app.emit_to(
                    PANEL,
                    "update-progress",
                    UpdateProgress { loaded: now, total },
                );
                let _ = progress_app.emit_to(PANEL, "notice", progress_text(now, total));
            },
            || {},
        )
        .await;
    match result {
        Ok(()) => {
            app.restart();
        }
        Err(e) => {
            slot.abort_install();
            Err(updater_err("无法安装更新：", e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{UpdateKind, UpdateSlot, check_with, progress_text, updater_user_text};

    #[test]
    fn unsupported_does_not_call_fetch() {
        let slot = UpdateSlot::new(false);
        let mut called = false;
        let status = check_with(&slot, || {
            called = true;
            Ok(Some("0.0.2".into()))
        })
        .unwrap();
        assert!(!called);
        assert!(!status.supported);
        assert_eq!(status.kind, UpdateKind::Idle);
        assert_eq!(status.version, None);
    }

    #[test]
    fn found_version_announced_only_once() {
        let slot = UpdateSlot::new(true);
        let status = check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        assert_eq!(status.kind, UpdateKind::Available);
        assert_eq!(status.version.as_deref(), Some("0.0.2"));
        assert_eq!(slot.take_announce(), Some("0.0.2".into()));
        assert_eq!(slot.take_announce(), None);
    }

    #[test]
    fn latest_remote_clears_available() {
        let slot = UpdateSlot::new(true);
        check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        let status = check_with(&slot, || Ok(None)).unwrap();
        assert_eq!(status.kind, UpdateKind::Idle);
        assert_eq!(status.version, None);
    }

    #[test]
    fn install_without_update_fails() {
        let slot = UpdateSlot::new(true);
        let err = slot.begin_install().unwrap_err();
        assert!(err.to_string().contains("没有可用更新"));
    }

    #[test]
    fn second_install_rejected() {
        let slot = UpdateSlot::new(true);
        check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        assert_eq!(slot.begin_install().unwrap(), "0.0.2");
        let err = slot.begin_install().unwrap_err();
        assert!(err.to_string().contains("正在升级"));
        assert_eq!(slot.status().kind, UpdateKind::Installing);
    }

    #[test]
    fn abort_install_returns_to_available() {
        let slot = UpdateSlot::new(true);
        check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        slot.begin_install().unwrap();
        slot.abort_install();
        let status = slot.status();
        assert_eq!(status.kind, UpdateKind::Available);
        assert_eq!(status.version.as_deref(), Some("0.0.2"));
    }

    #[test]
    fn updater_user_text_maps_missing_platform() {
        assert_eq!(
            updater_user_text(
                "无法检查更新：",
                r#"None of the fallback platforms ["darwin-aarch64-app", "darwin-aarch64"] were found in the response `platforms` object"#
            ),
            "无法检查更新：当前系统没有对应的更新包"
        );
        assert_eq!(
            updater_user_text("无法检查更新：", "timeout"),
            "无法检查更新：timeout"
        );
    }

    #[test]
    fn progress_text_with_and_without_total() {
        assert_eq!(progress_text(0, None), "正在下载更新…（升级会短暂退出）");
        assert_eq!(
            progress_text(12 * 1024 * 1024, Some(40 * 1024 * 1024)),
            "正在下载更新 12 / 40 MB"
        );
    }
}
