use serde::{Deserialize, Serialize};

use crate::{api::InputAction, core::Position};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TimedAction {
    pub action: InputAction,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestVector {
    pub structure: String,
    pub actions: Vec<TimedAction>,
    pub observe_min: Position,
    pub observe_max: Position,
    pub ticks: u64,
}
