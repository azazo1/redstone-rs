use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

use crate::{
    Action, BlockEntityChange, BlockEntityData, BlockEvent, BlockKindId, BlockPos, BlockStateId,
    DeferredBlockChange, DeferredRuleTask, Direction, GameTick, MicroStep, NeighborTask,
    NeighborUpdate, Probe, ProbeValue, RedstoneMode, ScheduledTick, SimulationPhase, SparseWorld,
    TickPriority, TraceEvent, TraceKind, WorldError, WorldEvent,
};

pub trait BlockRules: Send {
    fn version(&self) -> &str;

    fn air_state(&self) -> BlockStateId;

    fn block_kind(&self, state: BlockStateId) -> BlockKindId;

    fn block_name(&self, state: BlockStateId) -> &str;

    fn is_supported(&self, state: BlockStateId) -> bool;

    fn load_world(&mut self, _world: &SparseWorld) -> Result<(), RulesError> {
        Ok(())
    }

    fn initialize(
        &mut self,
        _ctx: &mut EventContext<'_>,
        _positions: &[BlockPos],
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn apply_action(
        &mut self,
        ctx: &mut EventContext<'_>,
        action: &Action,
    ) -> Result<(), RulesError>;

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
    ) -> Result<(), RulesError>;

    fn on_scheduled_tick(
        &mut self,
        ctx: &mut EventContext<'_>,
        tick: ScheduledTick,
    ) -> Result<(), RulesError>;

    fn on_block_event(
        &mut self,
        ctx: &mut EventContext<'_>,
        event: BlockEvent,
    ) -> Result<bool, RulesError>;

    fn on_deferred_task(
        &mut self,
        _ctx: &mut EventContext<'_>,
        task: DeferredRuleTask,
    ) -> Result<(), RulesError> {
        Err(RulesError::Message(format!(
            "unknown deferred rule task: {}",
            task.kind
        )))
    }

    fn tick_entities(&mut self, _ctx: &mut EventContext<'_>) -> Result<(), RulesError> {
        Ok(())
    }

    fn tick_block_entity(
        &mut self,
        _ctx: &mut EventContext<'_>,
        _pos: BlockPos,
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn read_probe(&self, world: &SparseWorld, probe: &Probe) -> ProbeValue;
}

pub struct EventContext<'a> {
    pub world: &'a mut SparseWorld,
    pub mode: RedstoneMode,
    pub tick: GameTick,
    pub phase: SimulationPhase,
    micro_step: &'a mut MicroStep,
    next_sub_tick_order: &'a mut i64,
    random_state: &'a mut u64,
    scheduled_ticks: &'a mut BTreeSet<ScheduledTick>,
    scheduled_keys: &'a mut BTreeSet<(BlockPos, BlockKindId)>,
    block_events: &'a mut VecDeque<BlockEvent>,
    block_event_keys: &'a mut BTreeSet<BlockEvent>,
    trace: &'a mut Vec<TraceEvent>,
    events: &'a mut Vec<WorldEvent>,
    neighbor_tasks: Vec<NeighborTask>,
}

impl<'a> EventContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        world: &'a mut SparseWorld,
        mode: RedstoneMode,
        tick: GameTick,
        phase: SimulationPhase,
        micro_step: &'a mut MicroStep,
        next_sub_tick_order: &'a mut i64,
        random_state: &'a mut u64,
        scheduled_ticks: &'a mut BTreeSet<ScheduledTick>,
        scheduled_keys: &'a mut BTreeSet<(BlockPos, BlockKindId)>,
        block_events: &'a mut VecDeque<BlockEvent>,
        block_event_keys: &'a mut BTreeSet<BlockEvent>,
        trace: &'a mut Vec<TraceEvent>,
        events: &'a mut Vec<WorldEvent>,
    ) -> Self {
        Self {
            world,
            mode,
            tick,
            phase,
            micro_step,
            next_sub_tick_order,
            random_state,
            scheduled_ticks,
            scheduled_keys,
            block_events,
            block_event_keys,
            trace,
            events,
            neighbor_tasks: Vec::new(),
        }
    }

    pub fn set_block(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        cause: impl Into<String>,
    ) -> Result<BlockStateId, RulesError> {
        let old_block_entity = self.world.block_entity(pos).cloned();
        let old_state = self.world.set_block(pos, state)?;
        if old_state != state {
            self.events.push(WorldEvent::Block {
                change: crate::BlockChange {
                    pos,
                    old_state,
                    new_state: state,
                },
            });
            self.push_trace(TraceKind::BlockChanged {
                pos,
                old_state,
                new_state: state,
                cause: cause.into(),
            });
        }
        if let Some(data) = old_block_entity
            && self.world.block_entity(pos).is_none()
        {
            self.events.push(WorldEvent::BlockEntity {
                change: BlockEntityChange::Remove { pos, data },
            });
        }
        Ok(old_state)
    }

    pub fn set_block_entity(&mut self, pos: BlockPos, data: BlockEntityData) {
        let old_data = self.world.block_entity(pos).cloned();
        if old_data.as_ref() == Some(&data) {
            return;
        }
        self.world.set_block_entity(pos, data.clone());
        let change = match old_data {
            Some(old_data) => BlockEntityChange::Update {
                pos,
                old_data,
                new_data: data,
            },
            None => BlockEntityChange::Create { pos, data },
        };
        self.events.push(WorldEvent::BlockEntity { change });
    }

    pub fn remove_block_entity(&mut self, pos: BlockPos) -> Option<BlockEntityData> {
        let data = self.world.remove_block_entity(pos)?;
        self.events.push(WorldEvent::BlockEntity {
            change: BlockEntityChange::Remove {
                pos,
                data: data.clone(),
            },
        });
        Some(data)
    }

    pub fn update_block_entity(
        &mut self,
        pos: BlockPos,
        update: impl FnOnce(&mut BlockEntityData),
    ) -> bool {
        let Some(old_data) = self.world.block_entity(pos).cloned() else {
            return false;
        };
        let Some(data) = self.world.block_entity_mut(pos) else {
            return false;
        };
        update(data);
        let new_data = data.clone();
        if old_data == new_data {
            return false;
        }
        self.events.push(WorldEvent::BlockEntity {
            change: BlockEntityChange::Update {
                pos,
                old_data,
                new_data,
            },
        });
        true
    }

    pub(crate) fn record_untracked_block_entity_changes(
        &mut self,
        before: &BTreeMap<BlockPos, BlockEntityData>,
    ) {
        let current = self
            .world
            .block_entities()
            .map(|(pos, data)| (*pos, data.clone()))
            .collect::<BTreeMap<_, _>>();
        let positions = before
            .keys()
            .chain(current.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        for pos in positions {
            let old_data = before.get(&pos);
            let new_data = current.get(&pos);
            if old_data == new_data || self.last_recorded_block_entity_data(pos) == Some(new_data) {
                continue;
            }
            let change = match (old_data, new_data) {
                (None, Some(data)) => BlockEntityChange::Create {
                    pos,
                    data: data.clone(),
                },
                (Some(old_data), Some(new_data)) => BlockEntityChange::Update {
                    pos,
                    old_data: old_data.clone(),
                    new_data: new_data.clone(),
                },
                (Some(data), None) => BlockEntityChange::Remove {
                    pos,
                    data: data.clone(),
                },
                (None, None) => continue,
            };
            self.events.push(WorldEvent::BlockEntity { change });
        }
    }

    fn last_recorded_block_entity_data(&self, pos: BlockPos) -> Option<Option<&BlockEntityData>> {
        self.events.iter().rev().find_map(|event| match event {
            WorldEvent::BlockEntity { change } if change.pos() == pos => {
                Some(change.current_data())
            }
            _ => None,
        })
    }

    pub fn schedule_tick(
        &mut self,
        pos: BlockPos,
        block: BlockKindId,
        delay: u64,
        priority: TickPriority,
    ) -> bool {
        let key = (pos, block);
        if !self.scheduled_keys.insert(key) {
            return false;
        }
        let tick = ScheduledTick {
            block,
            pos,
            trigger_tick: GameTick(self.tick.0.saturating_add(delay)),
            priority,
            sub_tick_order: *self.next_sub_tick_order,
        };
        *self.next_sub_tick_order += 1;
        self.scheduled_ticks.insert(tick);
        self.push_trace(TraceKind::ScheduledTickQueued {
            pos,
            block,
            trigger_tick: tick.trigger_tick,
            priority: priority.value(),
            sub_tick_order: tick.sub_tick_order,
        });
        true
    }

    pub fn has_scheduled_tick(&self, pos: BlockPos, block: BlockKindId) -> bool {
        self.scheduled_keys.contains(&(pos, block))
    }

    pub fn schedule_tick_after_neighbors(
        &mut self,
        pos: BlockPos,
        block: BlockKindId,
        delay: u64,
        priority: TickPriority,
    ) {
        self.neighbor_tasks
            .push(NeighborTask::ScheduleTickAfterNeighbors {
                pos,
                block,
                delay,
                priority,
            });
    }

    pub fn set_block_and_update_neighbors_after_neighbors(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        cause: impl Into<String>,
        source_block: BlockKindId,
    ) {
        self.neighbor_tasks
            .push(NeighborTask::SetBlockAndUpdateNeighborsAfterNeighbors {
                pos,
                state,
                cause: cause.into(),
                source_block,
            });
    }

    pub fn apply_block_changes_after_neighbors(
        &mut self,
        changes: Vec<DeferredBlockChange>,
        follow_up: Vec<NeighborTask>,
    ) {
        self.neighbor_tasks
            .push(NeighborTask::ApplyBlockChangesAfterNeighbors { changes, follow_up });
    }

    pub fn run_rule_task_after_neighbors(&mut self, task: DeferredRuleTask) {
        self.neighbor_tasks
            .push(NeighborTask::RunRuleTaskAfterNeighbors(task));
    }

    pub fn queue_block_event(&mut self, event: BlockEvent) -> bool {
        if !self.block_event_keys.insert(event) {
            return false;
        }
        self.block_events.push_back(event);
        self.push_trace(TraceKind::BlockEventQueued {
            pos: event.pos,
            block: event.block,
            param_a: event.param_a,
            param_b: event.param_b,
        });
        true
    }

    pub fn neighbor_changed(&mut self, update: NeighborUpdate) {
        self.neighbor_tasks.push(NeighborTask::Single(update));
    }

    pub fn update_neighbors(
        &mut self,
        source_pos: BlockPos,
        source_block: BlockKindId,
        skip_direction: Option<Direction>,
        orientation: Option<u8>,
    ) {
        self.neighbor_tasks.push(NeighborTask::Multi {
            source_pos,
            source_block,
            skip_direction,
            orientation,
            next_index: 0,
        });
    }

    pub fn unsupported(&mut self, pos: BlockPos, behavior: impl Into<String>) {
        self.push_trace(TraceKind::UnsupportedTrigger {
            pos,
            behavior: behavior.into(),
        });
    }

    pub fn random_bounded(&mut self, bound: u32) -> u32 {
        assert!(bound > 0, "random bound must be positive");
        if bound.is_power_of_two() {
            return ((bound as u64 * self.random_bits(31) as u64) >> 31) as u32;
        }
        loop {
            let bits = self.random_bits(31);
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) < 1 << 31 {
                return value;
            }
        }
    }

    fn random_bits(&mut self, bits: u32) -> u32 {
        const MULTIPLIER: u64 = 0x5deece66d;
        const ADDEND: u64 = 0xb;
        const MASK: u64 = (1 << 48) - 1;
        *self.random_state = self
            .random_state
            .wrapping_mul(MULTIPLIER)
            .wrapping_add(ADDEND)
            & MASK;
        (*self.random_state >> (48 - bits)) as u32
    }

    pub fn message(&mut self, level: impl Into<String>, message: impl Into<String>) {
        self.push_trace(TraceKind::Message {
            level: level.into(),
            message: message.into(),
        });
    }

    pub fn push_trace(&mut self, kind: TraceKind) {
        self.micro_step.0 += 1;
        self.trace.push(TraceEvent {
            tick: self.tick,
            micro_step: *self.micro_step,
            phase: self.phase,
            kind,
        });
    }

    pub(crate) fn take_neighbor_tasks(&mut self) -> Vec<NeighborTask> {
        std::mem::take(&mut self.neighbor_tasks)
    }
}

#[derive(Debug, Error)]
pub enum RulesError {
    #[error(transparent)]
    World(#[from] WorldError),
    #[error("不支持的方块行为: {block} at {pos:?}")]
    Unsupported { block: String, pos: BlockPos },
    #[error("无效方块状态: {0:?}")]
    InvalidState(BlockStateId),
    #[error("规则错误: {0}")]
    Message(String),
}
