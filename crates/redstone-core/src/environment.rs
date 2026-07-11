use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SimulationEnvironment {
    pub game_time: u64,
    pub overworld_time: u64,
    pub advance_time: bool,
    pub sky_light: u8,
}

impl Default for SimulationEnvironment {
    fn default() -> Self {
        Self {
            game_time: 0,
            overworld_time: 0,
            advance_time: true,
            sky_light: 15,
        }
    }
}
