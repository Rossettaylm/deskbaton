//! 交棒服务状态与设备配对相关的 Tauri 命令。

use std::sync::Arc;

use baton_net::{LinkSnapshot, TcpPeerLink, auth};
use serde::Serialize;
use tauri::State;

use crate::agent::AgentSlot;

#[derive(Serialize)]
pub struct AgentStatus {
    /// 本机设备名；服务未启动时为空。
    name: Option<String>,
    hotkey: Option<String>,
    error: Option<String>,
    /// 配置文件写了 `display.input_code`，不能重新识别本机接口。
    input_code_configured: bool,
}

#[tauri::command]
pub fn agent_status(slot: State<'_, AgentSlot>) -> AgentStatus {
    AgentStatus {
        name: slot
            .with_agent(|a| a.me().0.clone())
            .or_else(|| slot.name.clone()),
        hotkey: slot
            .with_agent(|a| a.hotkey().to_owned())
            .or_else(|| slot.hotkey.clone()),
        error: slot.error.clone(),
        input_code_configured: slot.input_code_configured(),
    }
}

/// 面板上点了某台已配对的电脑。
#[tauri::command]
pub fn handoff_to(slot: State<'_, AgentSlot>, peer: String) -> Result<(), String> {
    slot.handoff_to(&peer).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn relearn_input(slot: State<'_, AgentSlot>) -> Result<(), String> {
    slot.relearn_input().map_err(|e| e.to_string())
}

fn link(slot: &AgentSlot) -> Result<&Arc<TcpPeerLink>, String> {
    slot.link
        .as_ref()
        .ok_or_else(|| slot.error.clone().unwrap_or_else(|| "未能启动".to_owned()))
}

#[derive(Serialize)]
pub struct ClipboardPending {
    key: String,
    count: usize,
}

/// 各对端剪贴板发件箱里的条数。不含正文。
#[tauri::command]
pub fn clipboard_pending(slot: State<'_, AgentSlot>) -> Vec<ClipboardPending> {
    slot.clipboard_pending()
        .into_iter()
        .map(|(key, count)| ClipboardPending { key, count })
        .collect()
}

#[tauri::command]
pub fn list_devices(slot: State<'_, AgentSlot>) -> Result<LinkSnapshot, String> {
    Ok(link(&slot)?.snapshot())
}

#[tauri::command]
pub fn confirm_pairing(
    slot: State<'_, AgentSlot>,
    session: u64,
    accept: bool,
) -> Result<(), String> {
    link(&slot)?
        .confirm_pairing(session, accept)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pair_device(
    slot: State<'_, AgentSlot>,
    address: String,
    fingerprint: Option<String>,
) -> Result<(), String> {
    let fingerprint = match fingerprint {
        Some(hex) => Some(auth::parse_fingerprint(&hex).ok_or("设备指纹格式不对")?),
        None => None,
    };
    link(&slot)?.pair_with(address, fingerprint);
    Ok(())
}

#[tauri::command]
pub fn unpair_device(slot: State<'_, AgentSlot>, fingerprint: String) -> Result<(), String> {
    let fingerprint = auth::parse_fingerprint(&fingerprint).ok_or("设备指纹格式不对")?;
    link(&slot)?.unpair(&fingerprint).map_err(|e| e.to_string())
}
