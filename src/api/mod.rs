use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::info;

use crate::{
    core::{
        BlockEntity, BlockKind, BlockState, Position, Snapshot, TraceEvent, TraceFilter, TraceKind,
        World, WorldError,
    },
    io::StructureInput,
    minecraft::{toggle_lever, trigger_button},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputAction {
    pub tick: u64,
    pub operation: InputOperation,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum InputOperation {
    SetBlock {
        position: Position,
        state: BlockState,
    },
    UseBlock {
        position: Position,
    },
    SetComparatorSignal {
        position: Position,
        signal: u8,
    },
    SetPowered {
        position: Position,
        powered: bool,
    },
    TriggerNeighborUpdate {
        position: Position,
        changed_block: BlockKind,
    },
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error(transparent)]
    World(#[from] WorldError),
}

#[derive(Debug)]
pub struct SimulationSession {
    world: World,
    actions: BTreeMap<u64, Vec<InputAction>>,
}

impl SimulationSession {
    pub async fn load_structure(input: StructureInput) -> Result<Self, SessionError> {
        let mut world = World::new();
        let block_count = input.blocks.len();
        let progress_interval = (block_count / 10).max(1);
        info!(blocks = block_count, "loading structure blocks");
        for (index, block) in input.blocks.into_iter().enumerate() {
            world.set_state_loaded(block.position, block.state);
            if block_count >= 10_000 && (index + 1) % progress_interval == 0 {
                info!(loaded = index + 1, total = block_count, "structure load progress");
            }
        }
        info!(blocks = block_count, "initializing structure updates");
        let positions = world.snapshot().blocks;
        let initialization_total = positions.len();
        let initialization_interval = (initialization_total / 10).max(1);
        for (index, block) in positions.into_iter().enumerate() {
            world.neighbor_changed(block.position, block.state.kind);
            world.update_neighbors_at(block.position, block.state.kind);
            if initialization_total >= 10_000 && (index + 1) % initialization_interval == 0 {
                info!(initialized = index + 1, total = initialization_total, "structure initialization progress");
            }
        }
        world.clear_trace();
        info!(blocks = world.snapshot().blocks.len(), "structure initialized");
        Ok(Self {
            world,
            actions: BTreeMap::new(),
        })
    }

    pub async fn apply(&mut self, action: InputAction) {
        self.actions.entry(action.tick).or_default().push(action);
    }

    pub async fn step(&mut self) -> Result<(), SessionError> {
        let next_tick = self.world.game_tick().saturating_add(1);
        if let Some(actions) = self.actions.remove(&next_tick) {
            for action in actions {
                self.apply_now(action).await?;
            }
        }
        self.world.step();
        Ok(())
    }

    pub async fn run_until(&mut self, ticks: u64) {
        let progress_interval = (ticks / 10).max(1);
        for index in 0..ticks {
            if let Err(error) = self.step().await {
                tracing::error!(%error, "simulation step failed");
                break;
            }
            if ticks >= 100 && (index + 1) % progress_interval == 0 {
                info!(completed = index + 1, total = ticks, "simulation progress");
            }
        }
    }

    pub async fn snapshot(&self) -> Snapshot {
        self.world.snapshot()
    }

    pub async fn trace(&self) -> Vec<TraceEvent> {
        self.world.trace().to_vec()
    }

    pub async fn configure_trace(
        &mut self,
        filter: Option<TraceFilter>,
        enabled: Option<BTreeSet<TraceKind>>,
    ) {
        self.world.configure_trace(filter, enabled);
    }

    pub fn world(&self) -> &World {
        &self.world
    }

    async fn apply_now(&mut self, action: InputAction) -> Result<(), SessionError> {
        match action.operation {
            InputOperation::SetBlock { position, state } => {
                self.world.set_state(position, state, "input_set_block")?;
            }
            InputOperation::UseBlock { position } => match self.world.state(position).kind {
                BlockKind::Lever => toggle_lever(&mut self.world, position),
                BlockKind::Button => trigger_button(&mut self.world, position),
                _ => {}
            },
            InputOperation::SetComparatorSignal { position, signal } => {
                self.world.set_block_entity(
                    position,
                    Some(BlockEntity {
                        comparator_signal: signal.min(15),
                        piston_motion: None,
                    }),
                );
                self.world.update_neighbors_at(position, self.world.state(position).kind);
            }
            InputOperation::SetPowered { position, powered } => {
                crate::minecraft::set_external_power(&mut self.world, position, powered)?;
            }
            InputOperation::TriggerNeighborUpdate {
                position,
                changed_block,
            } => self.world.neighbor_changed(position, changed_block),
        }
        Ok(())
    }
}
