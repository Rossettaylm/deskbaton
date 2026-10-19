//! 程序内更新：状态槽、检查与安装。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use baton_core::{Error, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::PANEL;
use crate::agent::AgentSlot;
use crate::notify;

pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateKind {
    Idle,
    Available,
    Installing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UpdateStatus {
    pub kind: UpdateKind,
    pub version: Option<String>,
}

struct Inner {
    kind: UpdateKind,
    version: Option<String>,
    announced: Option<String>,
}

pub struct UpdateSlot {
    inner: Mutex<Inner>,
}

impl UpdateSlot {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                kind: UpdateKind::Idle,
                version: None,
                announced: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn status(&self) -> UpdateStatus {
        let inner = self.lock();
        UpdateStatus {
            kind: inner.kind,
            version: inner.version.clone(),
        }
    }

    fn apply_remote(&self, remote: Option<String>) -> UpdateStatus {
        let mut inner = self.lock();
        if inner.kind == UpdateKind::Installing {
            return UpdateStatus {
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
    if status.kind == UpdateKind::Installing {
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
    } else if lower.contains("could not fetch") {
        "检查失败，请稍后再试。".into()
    } else if lower.contains("timeout")
        || lower.contains("timed out")
        || lower.contains("error sending request")
    {
        format!("{prefix}网络超时，请检查能否访问 GitHub")
    } else {
        format!("{prefix}{text}")
    }
}

fn should_retry_check(text: &str) -> bool {
    text.to_ascii_lowercase().contains("could not fetch")
}

#[cfg(test)]
fn retry_if<T, E: ToString>(
    first: std::result::Result<T, E>,
    retry: impl FnOnce() -> std::result::Result<T, E>,
) -> std::result::Result<T, E> {
    match first {
        Ok(value) => Ok(value),
        Err(err) if should_retry_check(&err.to_string()) => retry(),
        Err(err) => Err(err),
    }
}

fn updater_err(prefix: &str, e: tauri_plugin_updater::Error) -> Error {
    Error::Config(updater_user_text(prefix, &e.to_string()))
}

const MANIFEST_URL: &str =
    "https://github.com/Rossettaylm/deskbaton/releases/latest/download/latest.json";

pub(crate) fn manifest_endpoint(now_unix: u64) -> String {
    format!("{MANIFEST_URL}?t={now_unix}")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn build_updater(
    app: &AppHandle,
    timeout: Duration,
) -> std::result::Result<tauri_plugin_updater::Updater, tauri_plugin_updater::Error> {
    let endpoint = manifest_endpoint(unix_now()).parse()?;
    app.updater_builder()
        .timeout(timeout)
        .header("Accept", "*/*")?
        .header("Cache-Control", "no-cache")?
        .endpoints(vec![endpoint])?
        .version_comparator(|current, release| {
            log::info!("本机 {current} / 清单 {}", release.version);
            release.version > current
        })
        .build()
}

fn updater_client(
    app: &AppHandle,
    timeout: Duration,
    prefix: &str,
) -> Result<tauri_plugin_updater::Updater> {
    build_updater(app, timeout).map_err(|e| updater_err(prefix, e))
}

fn log_checked(version: &Option<String>) {
    match version {
        Some(v) => log::info!("发现新版本 {v}"),
        None => log::info!("已是最新版本"),
    }
}

async fn check_once(
    app: &AppHandle,
) -> std::result::Result<Option<String>, tauri_plugin_updater::Error> {
    let updater = build_updater(app, CHECK_TIMEOUT)?;
    Ok(updater.check().await?.map(|u| u.version))
}

async fn fetch_remote(app: &AppHandle) -> Result<Option<String>> {
    log::info!("正在检查更新");
    let result = match check_once(app).await {
        Ok(version) => Ok(version),
        Err(e) if should_retry_check(&e.to_string()) => {
            log::info!("检查失败，重试一次：{e}");
            check_once(app).await
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(version) => {
            log_checked(&version);
            Ok(version)
        }
        Err(e) => Err(updater_err("无法检查更新：", e)),
    }
}

async fn check_remote(app: &AppHandle) -> Result<UpdateStatus> {
    let slot = app.state::<UpdateSlot>();
    let status = slot.status();
    if status.kind == UpdateKind::Installing {
        return Ok(status);
    }
    Ok(slot.apply_remote(fetch_remote(app).await?))
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
            if status.kind == UpdateKind::Idle {
                let _ = app.emit_to(PANEL, "notice", "已是最新版本");
            }
            announce(&app, false);
            Ok(status)
        }
        Err(e) => {
            log::info!("{e}");
            Err(cmd_err(e))
        }
    }
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> std::result::Result<(), String> {
    do_install(app).await.map_err(|e| {
        log::info!("{e}");
        cmd_err(e)
    })
}

async fn do_install(app: AppHandle) -> Result<()> {
    let slot = app.state::<UpdateSlot>();
    let _version = slot.begin_install()?;
    let _ = app.emit_to(PANEL, "notice", progress_text(0, None));
    let updater = updater_client(&app, INSTALL_TIMEOUT, "无法安装更新：")?;
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
    fn idle_check_always_fetches() {
        let slot = UpdateSlot::new();
        let mut called = false;
        let status = check_with(&slot, || {
            called = true;
            Ok(Some("0.0.2".into()))
        })
        .unwrap();
        assert!(called);
        assert_eq!(status.kind, UpdateKind::Available);
        assert_eq!(status.version.as_deref(), Some("0.0.2"));
    }

    #[test]
    fn found_version_announced_only_once() {
        let slot = UpdateSlot::new();
        let status = check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        assert_eq!(status.kind, UpdateKind::Available);
        assert_eq!(status.version.as_deref(), Some("0.0.2"));
        assert_eq!(slot.take_announce(), Some("0.0.2".into()));
        assert_eq!(slot.take_announce(), None);
    }

    #[test]
    fn latest_remote_clears_available() {
        let slot = UpdateSlot::new();
        check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        let status = check_with(&slot, || Ok(None)).unwrap();
        assert_eq!(status.kind, UpdateKind::Idle);
        assert_eq!(status.version, None);
    }

    #[test]
    fn install_without_update_fails() {
        let slot = UpdateSlot::new();
        let err = slot.begin_install().unwrap_err();
        assert!(err.to_string().contains("没有可用更新"));
    }

    #[test]
    fn second_install_rejected() {
        let slot = UpdateSlot::new();
        check_with(&slot, || Ok(Some("0.0.2".into()))).unwrap();
        assert_eq!(slot.begin_install().unwrap(), "0.0.2");
        let err = slot.begin_install().unwrap_err();
        assert!(err.to_string().contains("正在升级"));
        assert_eq!(slot.status().kind, UpdateKind::Installing);
    }

    #[test]
    fn abort_install_returns_to_available() {
        let slot = UpdateSlot::new();
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
            "无法检查更新：网络超时，请检查能否访问 GitHub"
        );
        assert_eq!(
            updater_user_text(
                "无法检查更新：",
                "error sending request for url (https://github.com/x): operation timed out"
            ),
            "无法检查更新：网络超时，请检查能否访问 GitHub"
        );
        assert_eq!(
            updater_user_text("无法检查更新：", "error sending request for url"),
            "无法检查更新：网络超时，请检查能否访问 GitHub"
        );
        assert_eq!(
            updater_user_text(
                "无法检查更新：",
                "Could not fetch a valid release JSON from the remote"
            ),
            "检查失败，请稍后再试。"
        );
    }

    #[test]
    fn fetch_json_failure_retries_once() {
        assert!(super::should_retry_check(
            "Could not fetch a valid release JSON from the remote"
        ));
        assert!(!super::should_retry_check("timeout"));
        assert!(!super::should_retry_check("update package signature"));
        let mut n = 0;
        let ok = super::retry_if(
            Err("Could not fetch a valid release JSON from the remote"),
            || {
                n += 1;
                Ok("0.0.5")
            },
        );
        assert_eq!(ok, Ok("0.0.5"));
        assert_eq!(n, 1);
        n = 0;
        let still_err: std::result::Result<(), &str> = super::retry_if(
            Err("Could not fetch a valid release JSON from the remote"),
            || {
                n += 1;
                Err("Could not fetch a valid release JSON from the remote")
            },
        );
        assert!(still_err.is_err());
        assert_eq!(n, 1);
        n = 0;
        let no_retry = super::retry_if(Err("timeout"), || {
            n += 1;
            Ok("nope")
        });
        assert_eq!(no_retry, Err("timeout"));
        assert_eq!(n, 0);
    }

    #[test]
    fn check_timeout_is_fifteen_seconds() {
        assert_eq!(super::CHECK_TIMEOUT, std::time::Duration::from_secs(15));
        let prod = include_str!("update.rs")
            .split("mod tests")
            .next()
            .expect("production source");
        assert!(
            prod.contains("build_updater(app, CHECK_TIMEOUT)")
                && prod.contains("build_updater(app, timeout)")
                && prod.contains(".timeout(timeout)"),
            "检查与下载必须带超时，否则 GitHub 卡住时按钮无响应"
        );
        assert!(
            !prod.contains("fetching.swap"),
            "自动检查未完成时不得立刻返回 idle 并宣称已是最新"
        );
    }

    #[test]
    fn manifest_endpoint_busts_cache() {
        let url = super::manifest_endpoint(1_792_338_000);
        assert_eq!(
            url,
            "https://github.com/Rossettaylm/deskbaton/releases/latest/download/latest.json?t=1792338000"
        );
    }

    #[test]
    fn check_avoids_json_accept_and_logs_both_versions() {
        let prod = include_str!("update.rs")
            .split("mod tests")
            .next()
            .expect("production source");
        assert!(
            prod.contains("Accept") && prod.contains("*/*"),
            "不得使用插件默认 Accept: application/json，Windows 上会误判已是最新"
        );
        assert!(
            prod.contains("Cache-Control") && prod.contains("no-cache"),
            "清单请求必须禁缓存"
        );
        assert!(
            prod.contains("本机") && prod.contains("清单"),
            "检查必须记下本机版本与清单版本"
        );
        assert!(
            prod.contains("version_comparator") && prod.contains("manifest_endpoint"),
            "比较与清单 URL 必须走带日志、带时间戳的路径"
        );
        assert!(
            !prod.contains("非安装版") && !prod.contains("supported"),
            "检查更新不按安装路径跳过"
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
