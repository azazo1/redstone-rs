use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::RedstoneMode;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    #[default]
    Auto,
    Interpreted,
    Compiled,
}

impl fmt::Display for ExecutionMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Auto => "auto",
            Self::Interpreted => "interpreted",
            Self::Compiled => "compiled",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackend {
    #[default]
    Interpreted,
    Compiled,
}

impl fmt::Display for ExecutionBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Interpreted => "interpreted",
            Self::Compiled => "compiled",
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionConfig {
    pub requested_mode: ExecutionMode,
    pub redstone_mode: RedstoneMode,
    pub trace_enabled: bool,
    pub record_events: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ExecutionReport {
    pub requested_mode: ExecutionMode,
    pub backend: ExecutionBackend,
    pub fallback_reason: Option<String>,
    pub compile_duration: Duration,
    pub node_count: usize,
    pub edge_count: usize,
    pub compiled_updates: u64,
    pub interpreted_updates: u64,
    pub partial_recompilations: u64,
    pub full_recompilations: u64,
    pub dense_full_rebuilds: u64,
    pub recompiled_nodes: u64,
    pub recompile_duration: Duration,
}

impl ExecutionReport {
    pub const fn total_updates(&self) -> u64 {
        self.compiled_updates
            .saturating_add(self.interpreted_updates)
    }

    pub fn compiled_hit_rate(&self) -> f64 {
        let total = self.total_updates();
        if total == 0 {
            return 0.0;
        }
        self.compiled_updates as f64 / total as f64
    }
}
