use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

use super::{
    BlockEvent, BlockKind, BlockState, NeighborUpdate, NeighborUpdater, Position,
    Scheduler, TickPriority, TraceEvent, TraceFilter, TraceKind, TraceRecorder,
};

const MAX_SCHEDULED_TICKS_PER_GAME_TICK: usize = 65_536;
const MAX_CHAINED_NEIGHBOR_UPDATES: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SectionPos {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl SectionPos {
    pub const fn from_block(position: Position) -> Self {
        Self {
            x: position.x.div_euclid(16),
            y: position.y.div_euclid(16),
            z: position.z.div_euclid(16),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockEntity {
    pub comparator_signal: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapshotBlock {
    pub position: Position,
    pub state: BlockState,
    pub block_entity: Option<BlockEntity>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub game_tick: u64,
    pub blocks: Vec<SnapshotBlock>,
}

#[derive(Debug, Error)]
pub enum WorldError {
    #[error("position {0} is not loaded")]
    Unloaded(Position),
}

#[derive(Debug, Default)]
struct Section {
    blocks: BTreeMap<u16, BlockState>,
}

impl Section {
    fn local_index(position: Position) -> u16 {
        let x = position.x.rem_euclid(16) as u16;
        let y = position.y.rem_euclid(16) as u16;
        let z = position.z.rem_euclid(16) as u16;
        x | (z << 4) | (y << 8)
    }

    fn get(&self, position: Position) -> BlockState {
        self.blocks.get(&Self::local_index(position)).copied().unwrap_or_default()
    }

    fn set(&mut self, position: Position, state: BlockState) {
        let index = Self::local_index(position);
        if state.kind == BlockKind::Air {
            self.blocks.remove(&index);
        } else {
            self.blocks.insert(index, state);
        }
    }
}

#[derive(Debug)]
pub struct World {
    sections: BTreeMap<SectionPos, Section>,
    loaded_sections: BTreeSet<SectionPos>,
    block_entities: BTreeMap<Position, BlockEntity>,
    scheduler: Scheduler,
    neighbors: NeighborUpdater,
    trace: TraceRecorder,
    game_tick: u64,
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}

impl World {
    pub fn new() -> Self {
        Self {
            sections: BTreeMap::new(),
            loaded_sections: BTreeSet::new(),
            block_entities: BTreeMap::new(),
            scheduler: Scheduler::default(),
            neighbors: NeighborUpdater::new(MAX_CHAINED_NEIGHBOR_UPDATES),
            trace: TraceRecorder::default(),
            game_tick: 0,
        }
    }

    pub fn game_tick(&self) -> u64 {
        self.game_tick
    }

    pub fn mark_loaded(&mut self, section: SectionPos) {
        self.loaded_sections.insert(section);
    }

    pub fn is_loaded(&self, position: Position) -> bool {
        self.loaded_sections.contains(&SectionPos::from_block(position))
    }

    pub fn state(&self, position: Position) -> BlockState {
        self.sections
            .get(&SectionPos::from_block(position))
            .map(|section| section.get(position))
            .unwrap_or_default()
    }

    pub fn block_entity(&self, position: Position) -> Option<&BlockEntity> {
        self.block_entities.get(&position)
    }

    pub fn set_block_entity(&mut self, position: Position, entity: Option<BlockEntity>) {
        match entity {
            Some(entity) => {
                self.block_entities.insert(position, entity);
            }
            None => {
                self.block_entities.remove(&position);
            }
        }
    }

    pub fn set_state(
        &mut self,
        position: Position,
        state: BlockState,
        cause: impl Into<String>,
    ) -> Result<bool, WorldError> {
        if !self.is_loaded(position) {
            return Err(WorldError::Unloaded(position));
        }
        let changed = self.set_state_silent(position, state, cause.into());
        if changed {
            self.enqueue_neighbor_update(NeighborUpdate::Multi {
                source: position,
                changed_block: state.kind,
                skip: None,
                next_index: 0,
            });
        }
        Ok(changed)
    }

    pub fn set_state_loaded(&mut self, position: Position, state: BlockState) {
        self.mark_loaded(SectionPos::from_block(position));
        self.set_state_silent(position, state, "structure_load".to_owned());
    }

    pub(crate) fn set_state_silent(&mut self, position: Position, state: BlockState, cause: String) -> bool {
        let section_pos = SectionPos::from_block(position);
        let section = self.sections.entry(section_pos).or_default();
        let before = section.get(position);
        if before == state {
            return false;
        }
        section.set(position, state);
        if state.kind == BlockKind::Air {
            self.block_entities.remove(&position);
        }
        self.trace.record(
            self.game_tick,
            TraceKind::StateChange,
            position,
            Some(before),
            Some(state),
            cause,
        );
        true
    }

    pub fn schedule_tick(
        &mut self,
        position: Position,
        block: BlockKind,
        delay: u64,
        priority: TickPriority,
    ) {
        self.scheduler.schedule(self.game_tick, position, block, delay, priority);
    }

    pub fn has_scheduled_tick(&self, position: Position, block: BlockKind) -> bool {
        self.scheduler.has_scheduled(position, block)
    }

    pub fn enqueue_block_event(&mut self, event: BlockEvent) {
        self.scheduler.enqueue_block_event(event);
    }

    pub fn neighbor_changed(&mut self, position: Position, changed_block: BlockKind) {
        self.enqueue_neighbor_update(NeighborUpdate::Single {
            position,
            changed_block,
            source: position,
        });
    }

    pub fn update_neighbors_at(&mut self, position: Position, changed_block: BlockKind) {
        self.enqueue_neighbor_update(NeighborUpdate::Multi {
            source: position,
            changed_block,
            skip: None,
            next_index: 0,
        });
    }

    fn enqueue_neighbor_update(&mut self, update: NeighborUpdate) {
        if !self.neighbors.enqueue(update) {
            warn!(tick = self.game_tick, "neighbor update limit reached");
            return;
        }
        if self.neighbors.begin_if_idle() {
            self.drain_neighbor_updates();
        }
    }

    fn drain_neighbor_updates(&mut self) {
        while let Some((position, changed_block, source)) = self.neighbors.take_next_event() {
            if !self.is_loaded(position) {
                continue;
            }
            self.trace.record(
                self.game_tick,
                TraceKind::NeighborUpdate,
                position,
                None,
                Some(self.state(position)),
                format!("neighbor:{changed_block:?}"),
            );
            crate::minecraft::handle_neighbor_changed(self, position, changed_block, source);
        }
        self.neighbors.finish();
    }

    pub fn step(&mut self) {
        self.game_tick = self.game_tick.saturating_add(1);
        let mut processed = 0usize;
        while processed < MAX_SCHEDULED_TICKS_PER_GAME_TICK {
            let Some(tick) = self.scheduler.pop_due(self.game_tick) else {
                break;
            };
            processed += 1;
            self.trace.record(
                self.game_tick,
                TraceKind::ScheduledTick,
                tick.position,
                Some(self.state(tick.position)),
                None,
                format!("scheduled:{:?}", tick.block),
            );
            crate::minecraft::handle_scheduled_tick(self, tick);
        }
        if processed == MAX_SCHEDULED_TICKS_PER_GAME_TICK {
            warn!(tick = self.game_tick, "scheduled tick cap reached");
        }

        while let Some(event) = self.scheduler.pop_block_event() {
            if !self.is_loaded(event.position) || self.state(event.position).kind != event.block {
                continue;
            }
            self.trace.record(
                self.game_tick,
                TraceKind::BlockEvent,
                event.position,
                Some(self.state(event.position)),
                None,
                format!("event:{}:{}", event.action, event.parameter),
            );
            crate::minecraft::handle_block_event(self, event);
        }
        debug!(tick = self.game_tick, pending = self.scheduler.pending_len(), "world tick completed");
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut blocks = Vec::new();
        for (section_pos, section) in &self.sections {
            for (index, state) in &section.blocks {
                let x = section_pos.x * 16 + (index & 15) as i32;
                let z = section_pos.z * 16 + ((index >> 4) & 15) as i32;
                let y = section_pos.y * 16 + ((index >> 8) & 15) as i32;
                let position = Position::new(x, y, z);
                blocks.push(SnapshotBlock {
                    position,
                    state: *state,
                    block_entity: self.block_entities.get(&position).cloned(),
                });
            }
        }
        Snapshot {
            game_tick: self.game_tick,
            blocks,
        }
    }

    pub fn trace(&self) -> &[TraceEvent] {
        self.trace.events()
    }

    pub fn configure_trace(&mut self, filter: Option<TraceFilter>, enabled: Option<BTreeSet<TraceKind>>) {
        self.trace.set_filter(filter, enabled);
    }

    pub fn clear_trace(&mut self) {
        self.trace.clear();
    }

    pub fn scheduled_count(&self) -> usize {
        self.scheduler.pending_len()
    }

    pub(crate) fn move_state_silent(&mut self, from: Position, to: Position, cause: String) {
        let state = self.state(from);
        self.set_state_silent(to, state, cause.clone());
        self.set_state_silent(from, BlockState::new(BlockKind::Air), cause);
    }
}
