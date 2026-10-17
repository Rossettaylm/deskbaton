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
}

#[tauri::command]
pub fn agent_status(slot: State<'_, AgentSlot>) -> AgentStatus {
    AgentStatus {
        name: slot.name.clone(),
        hotkey: slot.hotkey.clone(),
        error: slot.error.clone(),
    }
}

fn link(slot: &AgentSlot) -> Result<&Arc<TcpPeerLink>, String> {
    slot.link.as_ref().ok_or_else(|| {
        slot.error
            .clone()
            .unwrap_or_else(|| "交棒服务未启动".to_owned())
    })
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
