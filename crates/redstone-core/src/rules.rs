use std::collections::{BTreeSet, VecDeque};

use thiserror::Error;

use crate::{
    Action, BlockChange, BlockEntityChange, BlockEntityData, BlockEvent, BlockKindId, BlockPos,
    BlockStateId, DeferredBlockChange, DeferredRuleTask, Direction, ExecutionConfig,
    ExecutionReport, GameTick, MicroStep, NeighborTask, NeighborUpdate, Probe, ProbeValue,
    RedstoneMode, ScheduledTick, SimulationPhase, SparseWorld, TickPriority, TraceEvent, TraceKind,
    WorldError, WorldEvent,
};
use crate::scheduler::ScheduledTickQueue;

pub trait BlockRules: Send {
    fn version(&self) -> &str;

    fn air_state(&self) -> BlockStateId;

    fn block_kind(&self, state: BlockStateId) -> BlockKindId;

    fn block_name(&self, state: BlockStateId) -> &str;

    fn is_supported(&self, state: BlockStateId) -> bool;

    fn configure_execution(
        &mut self,
        _world: &SparseWorld,
        _config: ExecutionConfig,
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn prepare_execution(&mut self, _world: &SparseWorld) -> Result<(), RulesError> {
        Ok(())
    }

    fn synchronize_world(
        &mut self,
        _ctx: &mut EventContext<'_>,
        _changes: &[BlockChange],
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn execution_report(&self) -> ExecutionReport {
        ExecutionReport::default()
    }

    fn load_world(&mut self, _world: &SparseWorld) -> Result<(), RulesError> {
        Ok(())
    }

    fn begin_tick(&mut self, _tick: GameTick) {
    }

    fn initialize(
        &mut self,
        _ctx: &mut EventContext<'_>,
        _positions: &[BlockPos],
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn apply_update_side_effects(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
    ) -> Result<(), RulesError> {
        self.initialize(ctx, &[pos])?;
        let source_block = self.block_kind(ctx.world.get_block(pos));
        ctx.update_neighbors(pos, source_block, None, None);
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
        current_state: BlockStateId,
    ) -> Result<(), RulesError>;

    fn should_process_neighbor_update(
        &mut self,
        _update: NeighborUpdate,
        _current_state: BlockStateId,
    ) -> Result<bool, RulesError> {
        Ok(true)
    }

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

    fn should_tick_block_entity(
        &self,
        _world: &SparseWorld,
        _pos: BlockPos,
        _data: &BlockEntityData,
    ) -> bool {
        true
    }

    fn read_probe(&self, world: &SparseWorld, probe: &Probe) -> ProbeValue;
}

pub(crate) type NeighborTasks = Vec<NeighborTask>;

pub struct EventContext<'a> {
    pub world: &'a mut SparseWorld,
    pub mode: RedstoneMode,
    pub tick: GameTick,
    game_time: u64,
    overworld_time: u64,
    sky_light: u8,
    pub phase: SimulationPhase,
    micro_step: &'a mut MicroStep,
    next_sub_tick_order: &'a mut i64,
    random_state: &'a mut u64,
    scheduled_ticks: &'a mut ScheduledTickQueue,
    block_events: &'a mut VecDeque<BlockEvent>,
    block_event_keys: &'a mut BTreeSet<BlockEvent>,
    trace: Option<&'a mut Vec<TraceEvent>>,
    events: Option<&'a mut Vec<WorldEvent>>,
    neighbor_tasks: &'a mut NeighborTasks,
    block_changes: &'a mut Vec<BlockChange>,
    touched_block_entities: Vec<BlockPos>,
}

impl<'a> EventContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        world: &'a mut SparseWorld,
        mode: RedstoneMode,
        tick: GameTick,
        game_time: u64,
        overworld_time: u64,
        sky_light: u8,
        phase: SimulationPhase,
        micro_step: &'a mut MicroStep,
        next_sub_tick_order: &'a mut i64,
        random_state: &'a mut u64,
        scheduled_ticks: &'a mut ScheduledTickQueue,
        block_events: &'a mut VecDeque<BlockEvent>,
        block_event_keys: &'a mut BTreeSet<BlockEvent>,
        trace: Option<&'a mut Vec<TraceEvent>>,
        events: Option<&'a mut Vec<WorldEvent>>,
        neighbor_tasks: &'a mut NeighborTasks,
        block_changes: &'a mut Vec<BlockChange>,
    ) -> Self {
        block_changes.clear();
        Self {
            world,
            mode,
            tick,
            game_time,
            overworld_time,
            sky_light,
            phase,
            micro_step,
            next_sub_tick_order,
            random_state,
            scheduled_ticks,
            block_events,
            block_event_keys,
            trace,
            events,
            neighbor_tasks,
            block_changes,
            touched_block_entities: Vec::new(),
        }
    }

    pub const fn game_time(&self) -> u64 {
        self.game_time
    }

    pub const fn overworld_time(&self) -> u64 {
        self.overworld_time
    }

    pub const fn sky_light(&self) -> u8 {
        self.sky_light
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
            let change = BlockChange {
                pos,
                old_state,
                new_state: state,
            };
            self.record_block_change(change, cause);
            if old_block_entity.is_some() {
                self.touched_block_entities.push(pos);
            }
        }
        if let Some(data) = old_block_entity
            && self.world.block_entity(pos).is_none()
        {
            self.record_event(WorldEvent::BlockEntity {
                change: BlockEntityChange::Remove { pos, data },
            });
        }
        Ok(old_state)
    }

    pub fn set_block_state_only(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        cause: impl Into<String>,
    ) -> Result<BlockStateId, RulesError> {
        debug_assert!(self.world.block_entity(pos).is_none());
        let old_state = self.world.set_block(pos, state)?;
        debug_assert!(self.world.block_entity(pos).is_none());
        if old_state != state {
            self.record_block_change(
                BlockChange {
                    pos,
                    old_state,
                    new_state: state,
                },
                cause,
            );
        }
        Ok(old_state)
    }

    pub fn set_block_entity(&mut self, pos: BlockPos, data: BlockEntityData) {
        let old_data = self.world.block_entity(pos).cloned();
        if old_data.as_ref() == Some(&data) {
            return;
        }
        self.world.set_block_entity(pos, data.clone());
        self.touched_block_entities.push(pos);
        let change = match old_data {
            Some(old_data) => BlockEntityChange::Update {
                pos,
                old_data,
                new_data: data,
            },
            None => BlockEntityChange::Create { pos, data },
        };
        self.record_event(WorldEvent::BlockEntity { change });
    }

    pub fn remove_block_entity(&mut self, pos: BlockPos) -> Option<BlockEntityData> {
        let data = self.world.remove_block_entity(pos)?;
        self.touched_block_entities.push(pos);
        self.record_event(WorldEvent::BlockEntity {
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
        self.touched_block_entities.push(pos);
        self.record_event(WorldEvent::BlockEntity {
            change: BlockEntityChange::Update {
                pos,
                old_data,
                new_data,
            },
        });
        true
    }

    pub fn schedule_tick(
        &mut self,
        pos: BlockPos,
        block: BlockKindId,
        delay: u64,
        priority: TickPriority,
    ) -> bool {
        let tick = ScheduledTick {
            block,
            pos,
            trigger_tick: GameTick(self.tick.0.saturating_add(delay)),
            priority,
            sub_tick_order: *self.next_sub_tick_order,
        };
        if !self.scheduled_ticks.insert(tick) {
            return false;
        }
        *self.next_sub_tick_order += 1;
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
        self.scheduled_ticks.contains(pos, block)
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
                cause: cause.into().into_boxed_str(),
                source_block,
            });
    }

    pub fn apply_block_changes_after_neighbors(
        &mut self,
        changes: Vec<DeferredBlockChange>,
        follow_up: Vec<NeighborTask>,
    ) {
        self.neighbor_tasks
            .push(NeighborTask::ApplyBlockChangesAfterNeighbors {
                changes: changes.into_boxed_slice(),
                follow_up: follow_up.into_boxed_slice(),
            });
    }

    pub fn run_rule_task_after_neighbors(&mut self, task: DeferredRuleTask) {
        self.neighbor_tasks
            .push(NeighborTask::RunRuleTaskAfterNeighbors(Box::new(task)));
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
        let Some(trace) = self.trace.as_mut() else {
            return;
        };
        self.micro_step.0 += 1;
        trace.push(TraceEvent {
            tick: self.tick,
            micro_step: *self.micro_step,
            phase: self.phase,
            kind,
        });
    }

    pub(crate) fn take_touched_block_entities(&mut self) -> Vec<BlockPos> {
        std::mem::take(&mut self.touched_block_entities)
    }

    pub(crate) fn take_block_changes(&mut self) -> Vec<BlockChange> {
        std::mem::take(self.block_changes)
    }

    pub(crate) fn has_block_changes(&self) -> bool {
        !self.block_changes.is_empty()
    }

    pub(crate) fn recycle_block_changes(&mut self, mut changes: Vec<BlockChange>) {
        changes.clear();
        if self.block_changes.is_empty()
            && changes.capacity() > self.block_changes.capacity()
        {
            *self.block_changes = changes;
        }
    }

    fn record_block_change(
        &mut self,
        change: BlockChange,
        cause: impl Into<String>,
    ) {
        if let Some(events) = self.events.as_mut() {
            events.push(WorldEvent::Block {
                change: change.clone(),
            });
        }
        if self.trace.is_some() {
            self.push_trace(TraceKind::BlockChanged {
                pos: change.pos,
                old_state: change.old_state,
                new_state: change.new_state,
                cause: cause.into(),
            });
        }
        self.block_changes.push(change);
    }

    fn record_event(&mut self, event: WorldEvent) {
        if let Some(events) = self.events.as_mut() {
            events.push(event);
        }
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
