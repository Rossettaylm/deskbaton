use serde::{Deserialize, Serialize};

use crate::ids::HandoffId;

/// 本机在交棒流程中的状态。持棒方 = 当前同时拥有画面和键鼠的机器。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "handoff_id", rename_all = "snake_case")]
pub enum AgentState {
    Holding,
    Idle,
    HandingOff(HandoffId),
    Receiving(HandoffId),
}
