use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    api::{InputAction, SessionError, SimulationSession},
    core::{Position, Snapshot, SnapshotBlock, TraceEvent},
    io::StructureError,
};

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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SimulationTrace {
    pub frames: Vec<Snapshot>,
    pub events: Vec<TraceEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotDifference {
    pub tick: u64,
    pub position: Position,
    pub expected: Option<SnapshotBlock>,
    pub actual: Option<SnapshotBlock>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEventDifference {
    pub index: usize,
    pub expected: Option<TraceEvent>,
    pub actual: Option<TraceEvent>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationDifference {
    pub snapshots: Vec<SnapshotDifference>,
    pub events: Vec<TraceEventDifference>,
}

impl SimulationDifference {
    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty() && self.events.is_empty()
    }
}

#[derive(Debug, Error)]
pub enum VectorError {
    #[error("failed to read test vector: {0}")]
    Read(#[from] std::io::Error),
    #[error("invalid test vector json: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Structure(#[from] StructureError),
    #[error(transparent)]
    Session(#[from] SessionError),
}

impl TestVector {
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, VectorError> {
        let bytes = tokio::fs::read(path).await?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub async fn run_from_path(path: impl AsRef<Path>) -> Result<SimulationTrace, VectorError> {
        let path = path.as_ref();
        let vector = Self::from_path(path).await?;
        let root = path.parent().unwrap_or_else(|| Path::new("."));
        vector.run(root).await
    }

    pub async fn run(&self, root: impl AsRef<Path>) -> Result<SimulationTrace, VectorError> {
        let structure = crate::io::StructureInput::from_path(root.as_ref().join(&self.structure)).await?;
        let mut session = SimulationSession::load_structure(structure).await?;
        for action in &self.actions {
            session.apply(action.action.clone()).await;
        }
        let mut frames = Vec::with_capacity(self.ticks as usize);
        for _ in 0..self.ticks {
            session.step().await?;
            let mut frame = session.snapshot().await;
            frame.blocks.retain(|block| in_observation_area(block.position, self.observe_min, self.observe_max));
            frames.push(frame);
        }
        let events = session
            .trace()
            .await
            .into_iter()
            .filter(|event| in_observation_area(event.position, self.observe_min, self.observe_max))
            .collect();
        Ok(SimulationTrace {
            frames,
            events,
        })
    }
}

pub fn diff_snapshots(expected: &[Snapshot], actual: &[Snapshot]) -> Vec<SnapshotDifference> {
    let mut differences = Vec::new();
    let frame_count = expected.len().max(actual.len());
    for index in 0..frame_count {
        let expected_frame = expected.get(index);
        let actual_frame = actual.get(index);
        let tick = actual_frame
            .or(expected_frame)
            .map(|frame| frame.game_tick)
            .unwrap_or(index as u64 + 1);
        let expected_blocks = expected_frame.map_or_else(BTreeMap::new, indexed_blocks);
        let actual_blocks = actual_frame.map_or_else(BTreeMap::new, indexed_blocks);
        for position in expected_blocks.keys().chain(actual_blocks.keys()).copied().collect::<std::collections::BTreeSet<_>>() {
            let expected_block = expected_blocks.get(&position).cloned();
            let actual_block = actual_blocks.get(&position).cloned();
            if expected_block != actual_block {
                differences.push(SnapshotDifference {
                    tick,
                    position,
                    expected: expected_block,
                    actual: actual_block,
                });
            }
        }
    }
    differences
}

pub fn diff_traces(expected: &SimulationTrace, actual: &SimulationTrace) -> SimulationDifference {
    let event_count = expected.events.len().max(actual.events.len());
    let events = (0..event_count)
        .filter_map(|index| {
            let expected_event = expected.events.get(index).cloned();
            let actual_event = actual.events.get(index).cloned();
            (expected_event != actual_event).then_some(TraceEventDifference {
                index,
                expected: expected_event,
                actual: actual_event,
            })
        })
        .collect();
    SimulationDifference {
        snapshots: diff_snapshots(&expected.frames, &actual.frames),
        events,
    }
}

fn indexed_blocks(snapshot: &Snapshot) -> BTreeMap<Position, SnapshotBlock> {
    snapshot
        .blocks
        .iter()
        .cloned()
        .map(|block| (block.position, block))
        .collect()
}

fn in_observation_area(position: Position, min: Position, max: Position) -> bool {
    (min.x..=max.x).contains(&position.x)
        && (min.y..=max.y).contains(&position.y)
        && (min.z..=max.z).contains(&position.z)
}
