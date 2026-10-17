//! 交棒服务状态与设备配对相关的 Tauri 命令。

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
    let running = slot.with(|agent| (agent.me().0.clone(), agent.hotkey().to_owned()));
    AgentStatus {
        name: running.as_ref().map(|(name, _)| name.clone()),
        hotkey: running.map(|(_, hotkey)| hotkey),
        error: slot.error.clone(),
    }
}
