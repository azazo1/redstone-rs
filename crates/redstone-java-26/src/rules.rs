use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::hash::{BuildHasherDefault, Hash, Hasher};

use indexmap::IndexSet;
use redstone_core::{
    Action, BlockChange, BlockEntityData, BlockEvent, BlockKindId, BlockPos, BlockRules,
    BlockStateId, DeferredRuleTask, Direction, EntityId, EventContext, ExecutionConfig,
    ExecutionReport, NeighborUpdate, Probe, ProbeValue, RedstoneMode, RulesError, ScheduledTick,
    SparseWorld, TickPriority,
};
use rustc_hash::FxHashMap;
use tracing::debug;

use crate::compiled::{
    CompiledExecutor, CompiledOrderedWireEvent, CompiledWireTransition, NetworkInputPower,
    WirePlan,
};
use crate::orientation::{Orientation, SideBias};
use crate::{BlockBehavior, JAVA_VERSION, Java26Registry, PushReaction, StateDefinition};
use crate::environment::daylight_detector_power;

mod comparator;
mod consumer;
mod inventory;
mod item;
mod observer;
mod piston;
mod removal;
mod shape;

use inventory::block_entity_i64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Java26RuleCacheFingerprint {
    pub comparator_outputs: u64,
    pub torch_toggles: u64,
    pub event_counts: u64,
    pub hopper_last_tick: u64,
    pub active_hopper: u64,
}

pub struct Java26Rules {
    registry: Java26Registry,
    comparator_outputs: BTreeMap<BlockPos, u8>,
    block_signal_cache: HashMap<BlockPos, u8, BuildHasherDefault<BlockPosHasher>>,
    wire_kind: Option<BlockKindId>,
    torch_toggles: VecDeque<(u64, BlockPos)>,
    event_counts: BTreeMap<String, i64>,
    hopper_last_tick: BTreeMap<BlockPos, u64>,
    active_hopper: Option<BlockPos>,
    compiled: CompiledExecutor,
    compiled_wire_events: Vec<CompiledOrderedWireEvent>,
    compiled_wire_overlay_powers: Vec<u8>,
    compiled_wire_overlay_epochs: Vec<u32>,
    compiled_wire_overlay_epoch: u32,
    compiled_wire_overlay_order: Vec<u32>,
    compiled_wire_overlay_active: bool,
    wire_power_transitions: FxHashMap<(BlockStateId, u8), BlockStateId>,
}

impl Java26Rules {
    pub fn new(registry: Java26Registry) -> Self {
        let wire_kind = registry
            .states()
            .find(|state| matches!(state.behavior, BlockBehavior::Wire))
            .map(|state| state.kind);
        Self {
            registry,
            comparator_outputs: BTreeMap::new(),
            block_signal_cache: HashMap::default(),
            wire_kind,
            torch_toggles: VecDeque::new(),
            event_counts: BTreeMap::new(),
            hopper_last_tick: BTreeMap::new(),
            active_hopper: None,
            compiled: CompiledExecutor::default(),
            compiled_wire_events: Vec::new(),
            compiled_wire_overlay_powers: Vec::new(),
            compiled_wire_overlay_epochs: Vec::new(),
            compiled_wire_overlay_epoch: 0,
            compiled_wire_overlay_order: Vec::new(),
            compiled_wire_overlay_active: false,
            wire_power_transitions: FxHashMap::default(),
        }
    }

    pub fn registry(&self) -> &Java26Registry {
        &self.registry
    }

    pub fn semantic_cache_fingerprint(&self) -> Java26RuleCacheFingerprint {
        Java26RuleCacheFingerprint {
            comparator_outputs: hash_value(&self.comparator_outputs),
            torch_toggles: hash_value(&self.torch_toggles),
            event_counts: hash_value(&self.event_counts),
            hopper_last_tick: hash_value(&self.hopper_last_tick),
            active_hopper: hash_value(&self.active_hopper),
        }
    }

    pub fn comparator_output_cache(&self) -> &BTreeMap<BlockPos, u8> {
        &self.comparator_outputs
    }

    fn state(&self, state: BlockStateId) -> Result<&StateDefinition, RulesError> {
        self.registry
            .state(state)
            .ok_or(RulesError::InvalidState(state))
    }

    fn changed_state(
        &mut self,
        state: BlockStateId,
        property: &str,
        value: impl AsRef<str>,
    ) -> Result<BlockStateId, RulesError> {
        self.registry
            .with_property(state, property, value)
            .map_err(|error| RulesError::Message(error.to_string()))
    }

    fn wire_state_with_power(
        &mut self,
        state: BlockStateId,
        power: u8,
    ) -> Result<BlockStateId, RulesError> {
        let power = power.min(15);
        if let Some(next) = self.wire_power_transitions.get(&(state, power)).copied() {
            return Ok(next);
        }
        let next = self.changed_state(state, "power", power_value(power))?;
        self.wire_power_transitions.insert((state, power), next);
        Ok(next)
    }

    fn set_block(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
    ) -> Result<BlockStateId, RulesError> {
        let old = ctx.world.get_block(pos);
        let changed = old != state;
        let wire_to_wire = changed
            && self
                .state(old)
                .is_ok_and(|state| matches!(state.behavior, BlockBehavior::Wire))
            && self
                .state(state)
                .is_ok_and(|state| matches!(state.behavior, BlockBehavior::Wire));
        let old = if wire_to_wire {
            ctx.set_block_state_only(pos, state, cause)?
        } else {
            ctx.set_block(pos, state, cause)?
        };
        if changed && !wire_to_wire {
            self.block_signal_cache.clear();
        }
        Ok(old)
    }

    fn set_state_and_notify(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
        orientation: Option<u8>,
    ) -> Result<bool, RulesError> {
        let requested_state = state;
        let old = self.set_block(ctx, pos, requested_state, cause)?;
        if old == requested_state {
            return Ok(false);
        }
        let state = self.apply_observer_lifecycle(ctx, pos, old, requested_state, true, true)?;
        if state != requested_state {
            return Ok(true);
        }
        let source_block = self.registry.state(state).map_or_else(
            || self.registry.state(old).map(|state| state.kind),
            |state| Some(state.kind),
        );
        if let Some(source_block) = source_block {
            let changed_state = self.state(state)?.clone();
            if matches!(changed_state.behavior, BlockBehavior::Torch { .. }) {
                update_torch_output_neighbors(ctx, pos, &changed_state);
            }
            if let Some(orientation) = orientation {
                let orientation = Orientation::from_index(orientation);
                for direction in orientation.directions() {
                    if matches!(changed_state.behavior, BlockBehavior::Wire)
                        && !wire_connects_for_update(&changed_state, direction)
                    {
                        continue;
                    }
                    let neighbor_pos = pos.relative(direction);
                    let neighbor_orientation = orientation.with_front_preserve_up(direction);
                    ctx.neighbor_changed(NeighborUpdate {
                        pos: neighbor_pos,
                        source_pos: pos,
                        source_block,
                        orientation: Some(neighbor_orientation.index()),
                        moved_by_piston: false,
                    });
                    if self
                        .registry
                        .state(ctx.world.get_block(neighbor_pos))
                        .is_some_and(|state| state.redstone_conductor)
                    {
                        for secondary in neighbor_orientation.directions() {
                            if secondary != direction.opposite() {
                                ctx.neighbor_changed(NeighborUpdate {
                                    pos: neighbor_pos.relative(secondary),
                                    source_pos: neighbor_pos,
                                    source_block,
                                    orientation: Some(
                                        neighbor_orientation
                                            .with_front_preserve_up(secondary)
                                            .index(),
                                    ),
                                    moved_by_piston: false,
                                });
                            }
                        }
                    }
                }
            } else {
                match ctx.mode {
                    RedstoneMode::Default => ctx.update_neighbors(pos, source_block, None, None),
                    RedstoneMode::Experimental => {
                        let orientation = Orientation::from_index(ctx.random_bounded(48) as u8)
                            .with_side_bias(SideBias::Left);
                        for direction in Direction::UPDATE_ORDER {
                            ctx.neighbor_changed(NeighborUpdate {
                                pos: pos.relative(direction),
                                source_pos: pos,
                                source_block,
                                orientation: Some(orientation.with_front(direction).index()),
                                moved_by_piston: false,
                            });
                        }
                    }
                }
            }
        }
        self.queue_neighbor_shape_updates(ctx, pos);
        if self
            .registry
            .state(old)
            .is_some_and(comparator::has_analog_output)
            || self
                .registry
                .state(state)
                .is_some_and(comparator::has_analog_output)
        {
            self.refresh_comparators_near(ctx, pos)?;
        }
        Ok(true)
    }

    fn update_diode_output_neighbors(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) {
        let facing = state.facing.unwrap_or(Direction::North);
        let output_direction = facing.opposite();
        let output_pos = pos.relative(output_direction);
        let orientation = match ctx.mode {
            RedstoneMode::Default => None,
            RedstoneMode::Experimental => Some(
                Orientation::from_index(ctx.random_bounded(48) as u8)
                    .with_side_bias(SideBias::Left)
                    .with_up(Direction::Up)
                    .with_front(output_direction)
                    .index(),
            ),
        };
        ctx.neighbor_changed(NeighborUpdate {
            pos: output_pos,
            source_pos: pos,
            source_block: state.kind,
            orientation,
            moved_by_piston: false,
        });
        ctx.update_neighbors(output_pos, state.kind, Some(facing), orientation);
    }

    fn set_diode_state(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
    ) -> Result<bool, RulesError> {
        let old = self.set_block(ctx, pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        let state = self.apply_observer_lifecycle(ctx, pos, old, state, false, true)?;
        let definition = self.state(state)?.clone();
        self.update_neighbor_shapes(ctx, pos)?;
        self.update_diode_output_neighbors(ctx, pos, &definition);
        Ok(true)
    }

    fn set_state_and_update_shapes(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
    ) -> Result<bool, RulesError> {
        let old = self.set_block(ctx, pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        self.apply_observer_lifecycle(ctx, pos, old, state, false, true)?;
        self.update_neighbor_shapes(ctx, pos)?;
        Ok(true)
    }

    fn is_powered(&self, world: &SparseWorld, pos: BlockPos) -> bool {
        Direction::UPDATE_ORDER.into_iter().any(|direction| {
            let source_pos = pos.relative(direction);
            self.signal(world, source_pos, direction) > 0
        })
    }

    fn is_quasi_powered(&self, world: &SparseWorld, pos: BlockPos, facing: Direction) -> bool {
        Direction::UPDATE_ORDER.into_iter().any(|direction| {
            if direction == facing {
                return false;
            }
            let source_pos = pos.relative(direction);
            self.signal(world, source_pos, direction) > 0
        }) || Direction::UPDATE_ORDER.into_iter().any(|direction| {
            let above = pos.relative(Direction::Up);
            let source_pos = above.relative(direction);
            self.signal(world, source_pos, direction) > 0
        })
    }

    fn signal(&self, world: &SparseWorld, pos: BlockPos, direction: Direction) -> u8 {
        let state_id = world.get_block(pos);
        let Ok(state) = self.state(state_id) else {
            return 0;
        };
        let own_signal = self.weak_signal(world, pos, state, direction);
        if !state.redstone_conductor {
            return own_signal;
        }
        Direction::UPDATE_ORDER
            .into_iter()
            .map(|neighbor_direction| {
                self.direct_signal(world, pos.relative(neighbor_direction), neighbor_direction)
            })
            .max()
            .unwrap_or(0)
            .max(own_signal)
    }

    fn weak_signal(
        &self,
        _world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
        direction: Direction,
    ) -> u8 {
        match &state.behavior {
            BlockBehavior::RedstoneBlock => 15,
            BlockBehavior::Wire => {
                let power = self.visible_wire_power(pos, state);
                match direction {
                    Direction::Down => 0,
                    Direction::Up => power,
                    horizontal => {
                        let property = direction_name(horizontal.opposite());
                        if state
                            .property(property)
                            .is_none_or(|connection| connection != "none")
                        {
                            power
                        } else {
                            0
                        }
                    }
                }
            }
            BlockBehavior::Lever | BlockBehavior::Button { .. } => {
                if state.bool_property("powered") {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::Torch { wall } => {
                if !state.bool_property("lit") {
                    return 0;
                }
                if (*wall && state.facing == Some(direction))
                    || (!*wall && direction == Direction::Up)
                {
                    0
                } else {
                    15
                }
            }
            BlockBehavior::Repeater => {
                if state.bool_property("powered") && state.facing == Some(direction) {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::Comparator => {
                if state.facing == Some(direction) {
                    self.comparator_outputs.get(&pos).copied().unwrap_or(0)
                } else {
                    0
                }
            }
            BlockBehavior::Observer => {
                if state.bool_property("powered") && state.facing == Some(direction) {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::Target => state.int_property("power").unwrap_or(0).clamp(0, 15) as u8,
            BlockBehavior::PressurePlate { .. }
            | BlockBehavior::TripwireHook
            | BlockBehavior::DetectorRail
            | BlockBehavior::DaylightDetector => state
                .int_property("power")
                .unwrap_or_else(|| i32::from(state.bool_property("powered")) * 15)
                .clamp(0, 15) as u8,
            BlockBehavior::Lectern if state.bool_property("powered") => 15,
            BlockBehavior::TrappedChest => block_entity_i64(_world, pos, "open_count")
                .unwrap_or(0)
                .clamp(0, 15) as u8,
            _ => 0,
        }
    }

    fn direct_signal(&self, world: &SparseWorld, pos: BlockPos, direction: Direction) -> u8 {
        let state_id = world.get_block(pos);
        let Ok(state) = self.state(state_id) else {
            return 0;
        };
        match state.behavior {
            BlockBehavior::Torch { .. } if direction != Direction::Down => 0,
            BlockBehavior::Lever | BlockBehavior::Button { .. } => {
                if state.bool_property("powered") && attached_direction(state) == direction {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::PressurePlate { .. } => {
                if direction == Direction::Up {
                    self.weak_signal(world, pos, state, direction)
                } else {
                    0
                }
            }
            BlockBehavior::DetectorRail | BlockBehavior::Lectern | BlockBehavior::TrappedChest => {
                if direction == Direction::Up {
                    self.weak_signal(world, pos, state, direction)
                } else {
                    0
                }
            }
            BlockBehavior::TripwireHook => {
                if state.facing == Some(direction) {
                    self.weak_signal(world, pos, state, direction)
                } else {
                    0
                }
            }
            BlockBehavior::RedstoneBlock
            | BlockBehavior::Target
            | BlockBehavior::DaylightDetector
            | BlockBehavior::CopperBulb => 0,
            _ => self.weak_signal(world, pos, state, direction),
        }
    }

    fn signal_without_wire_feedback(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> u8 {
        let state_id = world.get_block(pos);
        let Ok(state) = self.state(state_id) else {
            return 0;
        };
        let own_signal = if matches!(state.behavior, BlockBehavior::Wire) {
            0
        } else {
            self.weak_signal(world, pos, state, direction)
        };
        if !state.redstone_conductor {
            return own_signal;
        }
        let mut signal = own_signal;
        for neighbor_direction in Direction::UPDATE_ORDER {
            let neighbor_pos = pos.relative(neighbor_direction);
            let Ok(neighbor) = self.state(world.get_block(neighbor_pos)) else {
                continue;
            };
            if !matches!(neighbor.behavior, BlockBehavior::Wire) {
                signal = signal.max(self.direct_signal(
                    world,
                    neighbor_pos,
                    neighbor_direction,
                ));
                if signal == 15 {
                    break;
                }
            }
        }
        signal
    }

    fn wire_target_power(&mut self, world: &SparseWorld, pos: BlockPos) -> u8 {
        if let Some(plan) = self.compiled.wire_plan(pos) {
            let block_power = self.compiled_wire_block_power(world, &plan);
            if block_power == 15 {
                return 15;
            }
            let wire_power = plan
                .wire_inputs
                .iter()
                .filter_map(|input| self.registry.state(world.get_block(*input)))
                .map(wire_power_of)
                .max()
                .unwrap_or(0);
            return block_power.max(wire_power.saturating_sub(1));
        }

        let block_power = if let Some(power) = self.block_signal_cache.get(&pos) {
            *power
        } else {
            let mut power = 0u8;
            for direction in Direction::UPDATE_ORDER {
                let neighbor = pos.relative(direction);
                power = power.max(
                    self.signal_without_wire_feedback(world, neighbor, direction),
                );
                if power == 15 {
                    break;
                }
            }
            self.block_signal_cache.insert(pos, power);
            power
        };
        if block_power == 15 {
            return 15;
        }

        let above_open = self
            .state(world.get_block(pos.relative(Direction::Up)))
            .map_or(true, |state| !state.redstone_conductor);
        let mut wire_power = 0u8;
        for horizontal in Direction::HORIZONTAL {
            let side = pos.relative(horizontal);
            let Ok(side_state) = self.state(world.get_block(side)) else {
                continue;
            };
            wire_power = wire_power.max(wire_power_of(side_state));
            if side_state.redstone_conductor && above_open {
                wire_power = wire_power.max(
                    self.state(world.get_block(side.relative(Direction::Up)))
                        .map_or(0, wire_power_of),
                );
            } else if !side_state.redstone_conductor {
                wire_power = wire_power.max(
                    self.state(world.get_block(side.relative(Direction::Down)))
                        .map_or(0, wire_power_of),
                );
            }
            if wire_power == 15 {
                break;
            }
        }
        block_power.max(wire_power.saturating_sub(1))
    }

    fn compiled_wire_block_power(
        &self,
        world: &SparseWorld,
        plan: &WirePlan,
    ) -> u8 {
        let mut block_power = 0;
        for input in &plan.block_inputs {
            let Some(input_state) = self.registry.state(world.get_block(input.pos)) else {
                continue;
            };
            let own_signal = if matches!(input_state.behavior, BlockBehavior::Wire) {
                0
            } else {
                self.weak_signal(world, input.pos, input_state, input.direction)
            };
            let mut input_power = own_signal.max(input.constant_power);
            if input_state.redstone_conductor {
                for source in &input.conductor_sources {
                    let Some(source_state) = self.registry.state(world.get_block(source.pos)) else {
                        continue;
                    };
                    if !matches!(source_state.behavior, BlockBehavior::Wire) {
                        input_power = input_power.max(self.direct_signal(
                            world,
                            source.pos,
                            source.direction,
                        ));
                    }
                    if input_power == 15 {
                        break;
                    }
                }
            }
            block_power = block_power.max(input_power);
            if block_power == 15 {
                break;
            }
        }
        block_power
    }

    fn refresh_compiled_electrical_target(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        behavior: BlockBehavior,
        input: Option<NetworkInputPower>,
    ) -> Result<bool, RulesError> {
        match behavior {
            BlockBehavior::Torch { .. } => {
                self.refresh_compiled_torch(ctx, pos, state_id, input)?
            }
            BlockBehavior::Repeater => {
                self.refresh_compiled_repeater(ctx, pos, state_id, input)?
            }
            BlockBehavior::Comparator => self.refresh_comparator(ctx, pos, state_id)?,
            BlockBehavior::Lamp
            | BlockBehavior::CopperBulb
            | BlockBehavior::PoweredConsumer
            | BlockBehavior::Door
            | BlockBehavior::PoweredRail
            | BlockBehavior::NoteBlock
            | BlockBehavior::Bell
            | BlockBehavior::Tnt => self.refresh_powered_consumer_with_input(
                ctx,
                pos,
                state_id,
                input.map(|input| input.default > 0),
            )?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    fn apply_compiled_ordered_wire_events(
        &mut self,
        ctx: &mut EventContext<'_>,
        events: &[CompiledOrderedWireEvent],
    ) -> Result<(u64, u64), RulesError> {
        let batch_writes = self.compiled.batch_wire_writes();
        if batch_writes {
            self.compiled_wire_overlay_epoch = self.compiled_wire_overlay_epoch.wrapping_add(1);
            if self.compiled_wire_overlay_epoch == 0 {
                self.compiled_wire_overlay_epochs.fill(0);
                self.compiled_wire_overlay_epoch = 1;
            }
            self.compiled_wire_overlay_order.clear();
        }
        self.compiled_wire_overlay_active = batch_writes;
        let mut wire_events = 0u64;
        let mut boundary_events = 0u64;
        let result = (|| {
            for event in events.iter().copied() {
                match event {
                    CompiledOrderedWireEvent::Wire(transition) => {
                        wire_events += 1;
                        let transition = self.compiled.resolve_wire_transition(transition)?;
                        if batch_writes {
                            self.stage_compiled_wire_transition(ctx, transition)?;
                        } else {
                            self.apply_compiled_wire_transition(ctx, transition)?;
                        }
                    }
                    CompiledOrderedWireEvent::Boundary {
                        target,
                        input,
                    } => {
                        boundary_events += 1;
                        let (pos, source_pos) = self.compiled.resolve_boundary(target)?;
                        let state_id = ctx.world.get_block(pos);
                        let (behavior, is_rail, is_piston_head, handles_neighbor_update) = {
                            let state = self.state(state_id)?;
                            (
                                state.behavior,
                                state.is_rail,
                                state.is_piston_head,
                                state.handles_neighbor_update,
                            )
                        };
                        if !self.refresh_compiled_electrical_target(
                            ctx,
                            pos,
                            state_id,
                            behavior,
                            input,
                        )? && (is_rail || is_piston_head || handles_neighbor_update)
                        {
                            ctx.neighbor_changed(NeighborUpdate {
                                pos,
                                source_pos,
                                source_block: self.wire_kind.unwrap_or(BlockKindId(0)),
                                orientation: None,
                                moved_by_piston: false,
                            });
                        }
                    }
                }
            }
            if batch_writes {
                self.flush_compiled_wire_overlay(ctx)?;
            }
            Ok(())
        })();
        self.compiled_wire_overlay_active = false;
        self.compiled_wire_overlay_order.clear();
        result.map(|()| (wire_events, boundary_events))
    }

    fn stage_compiled_wire_transition(
        &mut self,
        ctx: &mut EventContext<'_>,
        transition: CompiledWireTransition,
    ) -> Result<(), RulesError> {
        debug_assert_ne!(transition.old_power, transition.new_power);
        let wire = transition.wire as usize;
        if self.compiled_wire_overlay_powers.len() <= wire {
            self.compiled_wire_overlay_powers.resize(wire + 1, 0);
            self.compiled_wire_overlay_epochs.resize(wire + 1, 0);
        }
        let first_transition = self.compiled_wire_overlay_epochs[wire]
            != self.compiled_wire_overlay_epoch;
        self.compiled_wire_overlay_epochs[wire] = self.compiled_wire_overlay_epoch;
        self.compiled_wire_overlay_powers[wire] = transition.new_power;
        if first_transition {
            self.compiled_wire_overlay_order.push(transition.wire);
            self.update_compiled_wire_observers(ctx, &transition)?;
        }
        Ok(())
    }

    fn update_compiled_wire_observers(
        &mut self,
        ctx: &mut EventContext<'_>,
        transition: &CompiledWireTransition,
    ) -> Result<(), RulesError> {
        let end = transition.observer_start + u32::from(transition.observer_count);
        for index in transition.observer_start..end {
            let observer_pos = self.compiled.resolve_wire_observer(index)?;
            self.update_observer_shape(ctx, observer_pos, transition.pos)?;
        }
        Ok(())
    }

    fn flush_compiled_wire_overlay(
        &mut self,
        ctx: &mut EventContext<'_>,
    ) -> Result<(), RulesError> {
        let wires = std::mem::take(&mut self.compiled_wire_overlay_order);
        for wire in &wires {
            let index = *wire as usize;
            if self.compiled_wire_overlay_epochs[index] != self.compiled_wire_overlay_epoch {
                continue;
            }
            let power = self.compiled_wire_overlay_powers[index];
            let pos = self.compiled.resolve_wire_pos(*wire)?;
            let state_id = ctx.world.get_block(pos);
            let state = self.state(state_id)?;
            if !matches!(state.behavior, BlockBehavior::Wire) || state.power == power {
                continue;
            }
            let next = self.wire_state_with_power(state_id, power)?;
            self.set_block(ctx, pos, next, "compiled_network_wire_batch")?;
        }
        self.compiled_wire_overlay_order = wires;
        Ok(())
    }

    fn apply_compiled_wire_transition(
        &mut self,
        ctx: &mut EventContext<'_>,
        transition: CompiledWireTransition,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(transition.pos);
        let state = self.state(state_id)?.clone();
        if !matches!(state.behavior, BlockBehavior::Wire)
            || state.power == transition.new_power
        {
            return Ok(());
        }
        let next = self.changed_state(
            state_id,
            "power",
            power_value(transition.new_power),
        )?;
        self.set_block(ctx, transition.pos, next, "compiled_network_wire")?;
        self.update_compiled_wire_observers(ctx, &transition)?;
        Ok(())
    }

    fn update_wire(
        &mut self,
        ctx: &mut EventContext<'_>,
        initial_pos: BlockPos,
        incoming_orientation: Option<u8>,
    ) -> Result<(), RulesError> {
        match ctx.mode {
            RedstoneMode::Default => {
                let state_id = ctx.world.get_block(initial_pos);
                let state = self.state(state_id)?.clone();
                if !matches!(state.behavior, BlockBehavior::Wire) {
                    return Ok(());
                }
                let old_power = state.power;
                let new_power = self.wire_target_power(ctx.world, initial_pos);
                if old_power == new_power {
                    return Ok(());
                }
                let new_state = self.wire_state_with_power(state_id, new_power)?;
                self.set_block(ctx, initial_pos, new_state, "default_wire")?;
                self.update_observers_after_wire_power_change(ctx, initial_pos)?;
                for candidate in default_wire_update_positions(initial_pos) {
                    ctx.update_neighbors(candidate, state.kind, None, None);
                }
            }
            RedstoneMode::Experimental => {
                let orientation = incoming_orientation
                    .map(Orientation::from_index)
                    .unwrap_or_else(|| Orientation::from_index(ctx.random_bounded(48) as u8))
                    .with_up(Direction::Up)
                    .with_side_bias(SideBias::Left)
                    .index();
                let mut turn_off = VecDeque::from([(initial_pos, orientation)]);
                let mut turn_on = VecDeque::new();
                let mut known = IndexSet::new();
                known.insert(initial_pos);

                while let Some((pos, orientation)) = turn_off.pop_front() {
                    let state_id = ctx.world.get_block(pos);
                    let state = self.state(state_id)?.clone();
                    if !matches!(state.behavior, BlockBehavior::Wire) {
                        continue;
                    }
                    let old = state.power;
                    let target = self.wire_target_power(ctx.world, pos);
                    let next = if target < old { 0 } else { target };
                    if target > 0 && target < old {
                        turn_on.push_back((pos, orientation));
                    }
                    if next != old {
                        let new_state = self.wire_state_with_power(state_id, next)?;
                        self.set_state_and_notify(
                            ctx,
                            pos,
                            new_state,
                            "experimental_wire_off",
                            Some(orientation),
                        )?;
                    }
                    for (candidate, candidate_orientation) in
                        oriented_wire_neighbors(ctx.world, pos, orientation, &self.registry)
                    {
                        if known.insert(candidate) {
                            turn_off.push_back((candidate, candidate_orientation));
                        }
                    }
                }

                while let Some((pos, orientation)) = turn_on.pop_front() {
                    let state_id = ctx.world.get_block(pos);
                    let state = self.state(state_id)?.clone();
                    if !matches!(state.behavior, BlockBehavior::Wire) {
                        continue;
                    }
                    let old = state.power;
                    let target = self.wire_target_power(ctx.world, pos);
                    if target > old {
                        let new_state = self.wire_state_with_power(state_id, target)?;
                        self.set_state_and_notify(
                            ctx,
                            pos,
                            new_state,
                            "experimental_wire_on",
                            Some(orientation),
                        )?;
                        for (candidate, candidate_orientation) in
                            oriented_wire_neighbors(ctx.world, pos, orientation, &self.registry)
                        {
                            turn_on.push_back((candidate, candidate_orientation));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn refresh_torch(
        &self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?;
        if ctx.has_scheduled_tick(pos, state.kind) {
            return Ok(());
        }
        let attached = attached_block(pos, state);
        let should_be_lit = self.signal(ctx.world, attached, torch_input_direction(state)) == 0;
        if state.lit != should_be_lit {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_compiled_torch(
        &self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        input: Option<NetworkInputPower>,
    ) -> Result<(), RulesError> {
        let Some(input) = input else {
            return self.refresh_torch(ctx, pos, state_id);
        };
        let state = self.state(state_id)?;
        if ctx.has_scheduled_tick(pos, state.kind) {
            return Ok(());
        }
        let should_be_lit = input.default == 0;
        if state.lit != should_be_lit {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_repeater(
        &self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?;
        if ctx.has_scheduled_tick(pos, state.kind) {
            return Ok(());
        }
        if self.repeater_locked(ctx.world, pos, state) {
            return Ok(());
        }
        let should_power = self.diode_input(ctx.world, pos, state) > 0;
        if should_power != state.powered {
            let priority = if self.diode_should_prioritize(ctx.world, pos, state) {
                TickPriority::ExtremelyHigh
            } else if state.powered {
                TickPriority::VeryHigh
            } else {
                TickPriority::High
            };
            let delay = state.int_property("delay").unwrap_or(1).clamp(1, 4) as u64 * 2;
            ctx.schedule_tick(pos, state.kind, delay, priority);
        }
        Ok(())
    }

    fn refresh_compiled_repeater(
        &self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        input: Option<NetworkInputPower>,
    ) -> Result<(), RulesError> {
        let Some(input) = input else {
            return self.refresh_repeater(ctx, pos, state_id);
        };
        let state = self.state(state_id)?;
        if ctx.has_scheduled_tick(pos, state.kind) || input.side > 0 {
            return Ok(());
        }
        let should_power = input.default > 0;
        if should_power != state.powered {
            let priority = if self.diode_should_prioritize(ctx.world, pos, state) {
                TickPriority::ExtremelyHigh
            } else if state.powered {
                TickPriority::VeryHigh
            } else {
                TickPriority::High
            };
            let delay = state.int_property("delay").unwrap_or(1).clamp(1, 4) as u64 * 2;
            ctx.schedule_tick(pos, state.kind, delay, priority);
        }
        Ok(())
    }

    fn diode_input(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
        let facing = state.facing.unwrap_or(Direction::North);
        let rear = pos.relative(facing);
        let input = self.signal(world, rear, facing);
        let wire_power = self
            .state(world.get_block(rear))
            .ok()
            .filter(|state| matches!(state.behavior, BlockBehavior::Wire))
            .map_or(0, |state| self.visible_wire_power(rear, state));
        input.max(wire_power)
    }

    fn side_input(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
        let facing = state.facing.unwrap_or(Direction::North);
        [facing.clockwise(), facing.counter_clockwise()]
            .into_iter()
            .map(|direction| {
                let source = pos.relative(direction);
                self.control_input_signal(world, source, direction)
            })
            .max()
            .unwrap_or(0)
    }

    fn control_input_signal(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> u8 {
        let Ok(state) = self.state(world.get_block(pos)) else {
            return 0;
        };
        match state.behavior {
            BlockBehavior::RedstoneBlock => 15,
            BlockBehavior::Wire => self.visible_wire_power(pos, state),
            BlockBehavior::Lever
            | BlockBehavior::Button { .. }
            | BlockBehavior::Torch { .. }
            | BlockBehavior::Repeater
            | BlockBehavior::Comparator
            | BlockBehavior::Observer
            | BlockBehavior::Target
            | BlockBehavior::PressurePlate { .. }
            | BlockBehavior::TripwireHook
            | BlockBehavior::DetectorRail
            | BlockBehavior::DaylightDetector
            | BlockBehavior::Lectern
            | BlockBehavior::TrappedChest => self.direct_signal(world, pos, direction),
            _ => 0,
        }
    }

    fn visible_wire_power(&self, pos: BlockPos, state: &StateDefinition) -> u8 {
        if self.compiled_wire_overlay_active
            && let Some(wire) = self.compiled.wire_index(pos).map(|wire| wire as usize)
            && self.compiled_wire_overlay_epochs.get(wire).copied()
                == Some(self.compiled_wire_overlay_epoch)
        {
            return self.compiled_wire_overlay_powers[wire];
        }
        state.power
    }

    fn repeater_locked(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> bool {
        let facing = state.facing.unwrap_or(Direction::North);
        [facing.clockwise(), facing.counter_clockwise()]
            .into_iter()
            .any(|direction| {
                let source_pos = pos.relative(direction);
                let Ok(source) = self.state(world.get_block(source_pos)) else {
                    return false;
                };
                matches!(
                    source.behavior,
                    BlockBehavior::Repeater | BlockBehavior::Comparator
                ) && self.signal(world, source_pos, direction) > 0
            })
    }

    fn comparator_output(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
        let input = self.analog_input(world, pos, state);
        let side = self.side_input(world, pos, state);
        if side > input {
            0
        } else if state.property("mode") == Some("subtract") {
            input.saturating_sub(side)
        } else {
            input
        }
    }

    fn analog_input(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
        let facing = state.facing.unwrap_or(Direction::North);
        let rear = pos.relative(facing);
        let rear_state = self.state(world.get_block(rear)).ok();
        if rear_state.is_some_and(comparator::has_analog_output) {
            return self
                .analog_output(world, rear, facing.opposite())
                .unwrap_or(0);
        }
        let direct = self.diode_input(world, pos, state);
        if direct < 15 && rear_state.is_some_and(|state| state.redstone_conductor) {
            let far = rear.relative(facing);
            let far_state = self.state(world.get_block(far)).ok();
            let block_output = far_state
                .filter(|state| comparator::has_analog_output(state))
                .map(|_| {
                    self.analog_output(world, far, facing.opposite())
                        .unwrap_or(0)
                });
            let frame_output = item_frame_output(world, far, facing);
            let far_output = match (block_output, frame_output) {
                (Some(block), Some(frame)) => Some(block.max(frame)),
                (Some(block), None) => Some(block),
                (None, Some(frame)) => Some(frame),
                (None, None) => None,
            };
            if let Some(output) = far_output {
                return output;
            }
        }
        direct
    }

    fn analog_output(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> Option<u8> {
        comparator::analog_output(&self.registry, world, pos, direction)
    }

    fn refresh_comparator(
        &self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?;
        if ctx.has_scheduled_tick(pos, state.kind) {
            return Ok(());
        }
        let output = self.comparator_output(ctx.world, pos, state);
        let old = self.comparator_outputs.get(&pos).copied().unwrap_or(0);
        let should_power = output > 0;
        if old != output || state.powered != should_power {
            let priority = if self.diode_should_prioritize(ctx.world, pos, state) {
                TickPriority::High
            } else {
                TickPriority::Normal
            };
            ctx.schedule_tick(pos, state.kind, 2, priority);
        }
        Ok(())
    }

    fn cache_comparator_output(&mut self, pos: BlockPos, output: u8) {
        self.comparator_outputs.insert(pos, output);
        self.compiled.update_comparator_output(pos, output);
    }

    fn synchronize_comparator_cache(
        &mut self,
        world: &SparseWorld,
        changes: &[BlockChange],
    ) -> Result<(), RulesError> {
        for change in changes {
            let old_is_comparator = matches!(
                self.state(change.old_state)?.behavior,
                BlockBehavior::Comparator
            );
            let new_is_comparator = matches!(
                self.state(change.new_state)?.behavior,
                BlockBehavior::Comparator
            );
            match (old_is_comparator, new_is_comparator) {
                (true, false) => {
                    self.comparator_outputs.remove(&change.pos);
                    self.compiled.remove_comparator_output(change.pos);
                }
                (false, true) => {
                    let output = comparator_output_from_block_entity(world, change.pos);
                    self.cache_comparator_output(change.pos, output);
                }
                (false, false) | (true, true) => {}
            }
        }
        Ok(())
    }

    fn restore_interpreted_caches(&mut self, world: &SparseWorld) {
        self.block_signal_cache.clear();
        let outputs = self.compiled.comparator_outputs_snapshot();
        self.comparator_outputs.clear();
        for (pos, output) in outputs {
            let is_comparator = self
                .registry
                .state(world.get_block(pos))
                .is_some_and(|state| matches!(state.behavior, BlockBehavior::Comparator));
            if is_comparator {
                self.comparator_outputs.insert(pos, output);
            }
        }
        self.compiled
            .replace_comparator_outputs(&self.comparator_outputs);
    }

    fn diode_should_prioritize(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> bool {
        let output_direction = state.facing.unwrap_or(Direction::North).opposite();
        let output_state = self.state(world.get_block(pos.relative(output_direction)));
        output_state.is_ok_and(|output_state| {
            matches!(
                output_state.behavior,
                BlockBehavior::Repeater | BlockBehavior::Comparator
            ) && output_state.facing != Some(output_direction)
        })
    }

    fn refresh_triggered_container(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        notify_observers: bool,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = self.is_powered(ctx.world, pos)
            || matches!(
                state.behavior,
                BlockBehavior::Dropper | BlockBehavior::Dispenser
            ) && self.is_powered(ctx.world, pos.relative(Direction::Up));
        let triggered = state.bool_property("triggered");
        if powered && !triggered {
            let next = self.changed_state(state.id, "triggered", "true")?;
            ctx.schedule_tick(pos, state.kind, 4, TickPriority::Normal);
            if notify_observers {
                self.set_state_and_update_shapes(ctx, pos, next, "container_trigger")?;
            } else {
                self.set_block(ctx, pos, next, "container_trigger")?;
            }
        } else if !powered && triggered {
            let next = self.changed_state(state.id, "triggered", "false")?;
            if notify_observers {
                self.set_state_and_update_shapes(ctx, pos, next, "container_untrigger")?;
            } else {
                self.set_block(ctx, pos, next, "container_untrigger")?;
            }
        }
        Ok(())
    }

    fn refresh_powered_consumer(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        self.refresh_powered_consumer_with_input(ctx, pos, state_id, None)
    }

    fn refresh_powered_consumer_with_input(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        powered: Option<bool>,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = powered.unwrap_or_else(|| self.is_powered(ctx.world, pos));
        match state.behavior {
            BlockBehavior::Lamp => {
                if powered && !state.bool_property("lit") {
                    let next = self.changed_state(state_id, "lit", "true")?;
                    self.set_state_and_update_shapes(ctx, pos, next, "lamp_on")?;
                } else if !powered
                    && state.bool_property("lit")
                    && !ctx.has_scheduled_tick(pos, state.kind)
                {
                    ctx.schedule_tick(pos, state.kind, 4, TickPriority::Normal);
                }
            }
            BlockBehavior::CopperBulb => {
                let was_powered = state.bool_property("powered");
                if powered != was_powered {
                    let mut next = self.changed_state(state_id, "powered", powered.to_string())?;
                    if powered {
                        let lit = !state.bool_property("lit");
                        next = self.changed_state(next, "lit", lit.to_string())?;
                    }
                    self.set_state_and_notify(ctx, pos, next, "copper_bulb", None)?;
                }
            }
            BlockBehavior::PoweredConsumer => {
                if state.properties.contains_key("powered")
                    && state.bool_property("powered") != powered
                {
                    let next = self.changed_state(state_id, "powered", powered.to_string())?;
                    self.set_state_and_notify(ctx, pos, next, "powered_consumer", None)?;
                }
                if state.properties.contains_key("open") && state.bool_property("open") != powered {
                    let next = self.changed_state(state_id, "open", powered.to_string())?;
                    self.set_state_and_notify(ctx, pos, next, "powered_openable", None)?;
                }
            }
            BlockBehavior::Door => self.refresh_door(ctx, pos, state_id)?,
            BlockBehavior::PoweredRail => self.refresh_powered_rail(ctx, pos, state_id)?,
            BlockBehavior::NoteBlock | BlockBehavior::Bell => {
                self.refresh_edge_consumer(ctx, pos, state_id)?;
            }
            BlockBehavior::Tnt if powered => {
                self.set_state_and_notify(ctx, pos, self.registry.air_state(), "tnt_primed", None)?;
                ctx.unsupported(pos, "tnt_explosion");
                *self
                    .event_counts
                    .entry("tnt_primed".to_owned())
                    .or_default() += 1;
            }
            _ => {}
        }
        Ok(())
    }

    fn use_block(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        force_button: bool,
        force_lever: bool,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(pos);
        let state = self.state(state_id)?.clone();
        match state.behavior {
            BlockBehavior::Lever if !force_button => {
                let powered = !state.bool_property("powered");
                let next = self.changed_state(state_id, "powered", powered.to_string())?;
                if self.set_state_and_notify(ctx, pos, next, "lever_use", None)? {
                    update_attached_power_neighbors(ctx, pos, &state);
                }
            }
            BlockBehavior::Button { wooden } if !force_lever => {
                if !state.bool_property("powered") {
                    let next = self.changed_state(state_id, "powered", "true")?;
                    if self.set_state_and_notify(ctx, pos, next, "button_press", None)? {
                        update_attached_power_neighbors(ctx, pos, &state);
                    }
                    ctx.schedule_tick_after_neighbors(
                        pos,
                        state.kind,
                        if wooden { 30 } else { 20 },
                        TickPriority::Normal,
                    );
                }
            }
            BlockBehavior::Comparator if !force_button && !force_lever => {
                let mode = if state.property("mode") == Some("subtract") {
                    "compare"
                } else {
                    "subtract"
                };
                let next = self.changed_state(state_id, "mode", mode)?;
                self.set_state_and_notify(ctx, pos, next, "comparator_mode", None)?;
                self.refresh_comparator(ctx, pos, next)?;
            }
            BlockBehavior::Lectern if !force_button && !force_lever => {
                if !state.bool_property("powered") {
                    let next = self.changed_state(state_id, "powered", "true")?;
                    if self.set_state_and_notify(ctx, pos, next, "lectern_page_change", None)? {
                        ctx.update_neighbors(pos.relative(Direction::Down), state.kind, None, None);
                    }
                    ctx.schedule_tick_after_neighbors(pos, state.kind, 2, TickPriority::Normal);
                }
            }
            BlockBehavior::NoteBlock if !force_button && !force_lever => {
                let note = (state.int_property("note").unwrap_or(0) + 1).rem_euclid(25);
                let next = self.changed_state(state_id, "note", note.to_string())?;
                self.set_state_and_notify(ctx, pos, next, "note_block_use", None)?;
                *self
                    .event_counts
                    .entry("note_block_play".to_owned())
                    .or_default() += 1;
            }
            _ if force_button || force_lever => {
                return Err(RulesError::Message(format!(
                    "指定动作与方块不匹配: {} at {pos:?}",
                    state.name
                )));
            }
            _ => {}
        }
        Ok(())
    }
}

fn hash_value(value: &impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn comparator_output_from_block_entity(world: &SparseWorld, pos: BlockPos) -> u8 {
    world
        .block_entity(pos)
        .map_or(0, comparator_output_from_data)
}

fn comparator_output_from_data(data: &BlockEntityData) -> u8 {
    data.fields
        .get("comparator_output")
        .or_else(|| data.fields.get("OutputSignal"))
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        .clamp(0, 15) as u8
}

#[derive(Default)]
struct BlockPosHasher(u64);

impl Hasher for BlockPosHasher {
    fn finish(&self) -> u64 {
        let mut value = self.0;
        value ^= value >> 30;
        value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value ^= value >> 27;
        value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = self
                .0
                .wrapping_mul(0x100_0000_01b3)
                .wrapping_add(u64::from(*byte));
        }
    }

    fn write_i32(&mut self, value: i32) {
        self.0 = self
            .0
            .rotate_left(21)
            .wrapping_add(u64::from(value as u32).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    }
}

impl BlockRules for Java26Rules {
    fn version(&self) -> &str {
        JAVA_VERSION
    }

    fn air_state(&self) -> BlockStateId {
        self.registry.air_state()
    }

    fn block_kind(&self, state: BlockStateId) -> BlockKindId {
        self.registry
            .state(state)
            .map_or(BlockKindId(0), |state| state.kind)
    }

    fn block_name(&self, state: BlockStateId) -> &str {
        self.registry
            .state(state)
            .map_or("minecraft:unknown", |state| state.name.as_ref())
    }

    fn is_supported(&self, state: BlockStateId) -> bool {
        self.registry
            .state(state)
            .is_some_and(|state| state.supported)
    }

    fn configure_execution(
        &mut self,
        world: &SparseWorld,
        config: ExecutionConfig,
    ) -> Result<(), RulesError> {
        self.compiled.configure(&self.registry, world, config)
    }

    fn prepare_execution(&mut self, world: &SparseWorld) -> Result<(), RulesError> {
        self.compiled.prepare(&self.registry, world)
    }

    fn synchronize_world(
        &mut self,
        ctx: &mut EventContext<'_>,
        changes: &[BlockChange],
    ) -> Result<(), RulesError> {
        self.synchronize_comparator_cache(ctx.world, changes)?;
        let synchronization = self
            .compiled
            .synchronize(&self.registry, ctx.world, changes)?;
        if synchronization.full_recompile || synchronization.recompiled_nodes > 0 {
            debug!(
                full = synchronization.full_recompile,
                recompiled_nodes = synchronization.recompiled_nodes,
                "同步编译红石拓扑"
            );
        }
        if synchronization.fallback_to_interpreted {
            self.restore_interpreted_caches(ctx.world);
            debug!("编译执行器回退后已恢复解释器缓存");
        }
        Ok(())
    }

    fn execution_report(&self) -> ExecutionReport {
        self.compiled.report()
    }

    fn load_world(&mut self, world: &SparseWorld) -> Result<(), RulesError> {
        self.block_signal_cache.clear();
        self.comparator_outputs.clear();
        self.hopper_last_tick.clear();
        for (pos, data) in world.block_entities() {
            let state = self.state(world.get_block(*pos))?;
            if !matches!(state.behavior, BlockBehavior::Comparator) {
                continue;
            }
            let output = comparator_output_from_data(data);
            self.comparator_outputs.insert(*pos, output);
        }
        self.compiled
            .replace_comparator_outputs(&self.comparator_outputs);
        Ok(())
    }

    fn begin_tick(&mut self, _tick: redstone_core::GameTick) {
        self.block_signal_cache.clear();
        self.active_hopper = None;
    }

    fn initialize(
        &mut self,
        ctx: &mut EventContext<'_>,
        positions: &[BlockPos],
    ) -> Result<(), RulesError> {
        for pos in positions {
            self.repair_shape(ctx, *pos, false)?;
        }
        for pos in positions {
            let state_id = ctx.world.get_block(*pos);
            let state = self.state(state_id)?.clone();
            match state.behavior {
                BlockBehavior::Wire => self.update_wire(ctx, *pos, None)?,
                BlockBehavior::Torch { .. } => self.refresh_torch(ctx, *pos, state_id)?,
                BlockBehavior::Repeater => self.refresh_repeater(ctx, *pos, state_id)?,
                BlockBehavior::Comparator => self.refresh_comparator(ctx, *pos, state_id)?,
                BlockBehavior::Hopper => self.refresh_hopper_enabled(ctx, *pos, state_id)?,
                BlockBehavior::Piston { .. } => self.refresh_piston(ctx, *pos, state_id)?,
                BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                    self.refresh_triggered_container(ctx, *pos, state_id, false)?
                }
                BlockBehavior::Lamp
                | BlockBehavior::CopperBulb
                | BlockBehavior::PoweredConsumer
                | BlockBehavior::Door
                | BlockBehavior::PoweredRail
                | BlockBehavior::NoteBlock
                | BlockBehavior::Bell
                | BlockBehavior::Tnt => self.refresh_powered_consumer(ctx, *pos, state_id)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn apply_update_side_effects(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
    ) -> Result<(), RulesError> {
        self.initialize(ctx, &[pos])?;
        self.update_neighbor_shapes(ctx, pos)?;
        let source_block = self.block_kind(ctx.world.get_block(pos));
        ctx.update_neighbors(pos, source_block, None, None);
        Ok(())
    }

    fn apply_action(
        &mut self,
        ctx: &mut EventContext<'_>,
        action: &Action,
    ) -> Result<(), RulesError> {
        self.block_signal_cache.clear();
        match action {
            Action::SetBlock { pos, state } => {
                let state = if self
                    .state(*state)
                    .is_ok_and(|state| matches!(state.behavior, BlockBehavior::Hopper))
                {
                    let enabled = !self.is_powered(ctx.world, *pos);
                    self.changed_state(*state, "enabled", enabled.to_string())?
                } else {
                    *state
                };
                self.set_state_and_notify(ctx, *pos, state, "action_set_block", None)?;
                self.repair_shape(ctx, *pos, true)?;
            }
            Action::BreakBlock { pos } => {
                self.set_state_and_notify(
                    ctx,
                    *pos,
                    self.registry.air_state(),
                    "action_break_block",
                    None,
                )?;
            }
            Action::UseBlock { pos } => self.use_block(ctx, *pos, false, false)?,
            Action::PressButton { pos } => self.use_block(ctx, *pos, true, false)?,
            Action::PullLever { pos } => self.use_block(ctx, *pos, false, true)?,
            Action::SetBlockEntity { pos, data } => {
                ctx.set_block_entity(*pos, data.clone());
                self.refresh_comparators_near(ctx, *pos)?;
            }
            Action::SpawnEntity { id, data } => {
                if let Some(id) = id {
                    ctx.world.spawn_entity_with_id(*id, data.clone())?;
                } else {
                    ctx.world.spawn_entity(data.clone());
                }
                self.refresh_comparators_near(ctx, entity_block_pos(data.position))?;
            }
            Action::MoveEntity { id, position } => {
                let old = ctx
                    .world
                    .entity(*id)
                    .map(|entity| entity_block_pos(entity.position))
                    .ok_or_else(|| RulesError::Message(format!("实体不存在: {id:?}")))?;
                if !ctx.world.move_entity(*id, *position) {
                    return Err(RulesError::Message(format!("实体不存在: {id:?}")));
                }
                self.refresh_comparators_near(ctx, old)?;
                self.refresh_comparators_near(ctx, entity_block_pos(*position))?;
            }
            Action::RemoveEntity { id } => {
                let entity = ctx
                    .world
                    .remove_entity(*id)
                    .ok_or_else(|| RulesError::Message(format!("实体不存在: {id:?}")))?;
                self.refresh_comparators_near(ctx, entity_block_pos(entity.position))?;
            }
            Action::SetEntityField { id, field, value } => {
                let pos = ctx
                    .world
                    .entity(*id)
                    .map(|entity| entity_block_pos(entity.position))
                    .ok_or_else(|| RulesError::Message(format!("实体不存在: {id:?}")))?;
                ctx.world
                    .entity_fields_mut(*id)
                    .expect("entity existence checked")
                    .insert(field.clone(), value.clone());
                self.refresh_comparators_near(ctx, pos)?;
            }
            Action::HitTarget {
                pos,
                face,
                location,
                arrow,
            } => self.hit_target(ctx, *pos, *face, *location, *arrow)?,
        }
        Ok(())
    }

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
        current_state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let current_state = self.state(current_state_id)?;
        let loses_support = current_state.is_rail
            && self.rail_support_changed(update.pos, current_state, update.source_pos)
            && !self.rail_survives(ctx.world, update.pos, current_state);
        if loses_support {
            self.remove_block_after_support_loss(ctx, update.pos, "neighbor_support_loss")?;
            return Ok(());
        }
        let state_id = current_state_id;
        let behavior = current_state.behavior;
        let piston_head_state = current_state
            .is_piston_head
            .then(|| current_state.clone());
        if let Some(state) = piston_head_state.as_ref() {
            self.refresh_piston_head(ctx, update.pos, state, update)?;
        }
        match behavior {
            BlockBehavior::Wire => {
                let mut events = std::mem::take(&mut self.compiled_wire_events);
                let propagated = self.compiled.propagate_ordered(update.pos, &mut events);
                let mut applied_counts = None;
                let result = match propagated {
                    Ok(true) => match self.apply_compiled_ordered_wire_events(ctx, &events) {
                        Ok(counts) => {
                            applied_counts = Some(counts);
                            Ok(())
                        }
                        Err(error) => match self.compiled.fail_ordered_application(error) {
                            Ok(()) => {
                                self.restore_interpreted_caches(ctx.world);
                                self.update_wire(ctx, update.pos, update.orientation)
                            }
                            Err(error) => Err(error),
                        },
                    },
                    Ok(false) => {
                        self.restore_interpreted_caches(ctx.world);
                        self.update_wire(ctx, update.pos, update.orientation)
                    }
                    Err(error) => Err(error),
                };
                if result.is_ok()
                    && let Some((wire, boundary)) = applied_counts
                {
                    self.compiled.record_ordered_events(wire, boundary);
                }
                events.clear();
                self.compiled_wire_events = events;
                result?;
            }
            BlockBehavior::Torch { .. } => self.refresh_torch(ctx, update.pos, state_id)?,
            BlockBehavior::Repeater => self.refresh_repeater(ctx, update.pos, state_id)?,
            BlockBehavior::Comparator => self.refresh_comparator(ctx, update.pos, state_id)?,
            BlockBehavior::Observer => {}
            BlockBehavior::Hopper => {
                self.refresh_hopper_enabled(ctx, update.pos, state_id)?
            }
            BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                self.refresh_triggered_container(ctx, update.pos, state_id, true)?
            }
            BlockBehavior::Piston { .. } => self.refresh_piston(ctx, update.pos, state_id)?,
            BlockBehavior::Lamp
            | BlockBehavior::CopperBulb
            | BlockBehavior::PoweredConsumer
            | BlockBehavior::Door
            | BlockBehavior::PoweredRail
            | BlockBehavior::NoteBlock
            | BlockBehavior::Bell
            | BlockBehavior::Tnt => self.refresh_powered_consumer(ctx, update.pos, state_id)?,
            _ => {}
        }
        Ok(())
    }

    fn should_process_neighbor_update(
        &mut self,
        update: NeighborUpdate,
        current_state_id: BlockStateId,
    ) -> Result<bool, RulesError> {
        if Some(update.source_block) != self.wire_kind {
            self.block_signal_cache.clear();
        }
        let current_state = self.state(current_state_id)?;
        Ok(current_state.is_rail
            || current_state.is_piston_head
            || current_state.handles_neighbor_update)
    }

    fn on_scheduled_tick(
        &mut self,
        ctx: &mut EventContext<'_>,
        tick: ScheduledTick,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(tick.pos);
        let state = self.state(state_id)?.clone();
        if state.kind != tick.block {
            return Ok(());
        }
        match state.behavior {
            BlockBehavior::Button { .. } => {
                if state.bool_property("powered") {
                    let next = self.changed_state(state_id, "powered", "false")?;
                    if self.set_state_and_notify(ctx, tick.pos, next, "button_release", None)? {
                        update_attached_power_neighbors(ctx, tick.pos, &state);
                    }
                }
            }
            BlockBehavior::Torch { .. } => {
                let should_be_lit = self.signal(
                    ctx.world,
                    attached_block(tick.pos, &state),
                    torch_input_direction(&state),
                ) == 0;
                if should_be_lit != state.bool_property("lit") {
                    while self
                        .torch_toggles
                        .front()
                        .is_some_and(|(toggle_tick, _)| toggle_tick.saturating_add(60) < ctx.tick.0)
                    {
                        self.torch_toggles.pop_front();
                    }
                    let burnout = !should_be_lit
                        && self
                            .torch_toggles
                            .iter()
                            .filter(|(_, pos)| *pos == tick.pos)
                            .count()
                            >= 7;
                    let next = self.changed_state(state_id, "lit", should_be_lit.to_string())?;
                    self.set_state_and_notify(ctx, tick.pos, next, "redstone_torch", None)?;
                    if !should_be_lit {
                        self.torch_toggles.push_back((ctx.tick.0, tick.pos));
                    }
                    if burnout {
                        ctx.schedule_tick(tick.pos, state.kind, 160, TickPriority::Normal);
                        *self
                            .event_counts
                            .entry("torch_burnout".to_owned())
                            .or_default() += 1;
                    }
                }
            }
            BlockBehavior::Repeater => {
                if !self.repeater_locked(ctx.world, tick.pos, &state) {
                    let should_power = self.diode_input(ctx.world, tick.pos, &state) > 0;
                    if state.bool_property("powered") && !should_power {
                        let next = self.changed_state(state_id, "powered", "false")?;
                        self.set_diode_state(ctx, tick.pos, next, "repeater_tick")?;
                    } else if !state.bool_property("powered") {
                        let next = self.changed_state(state_id, "powered", "true")?;
                        self.set_diode_state(ctx, tick.pos, next, "repeater_tick")?;
                        if !should_power {
                            let delay =
                                state.int_property("delay").unwrap_or(1).clamp(1, 4) as u64 * 2;
                            ctx.schedule_tick(tick.pos, state.kind, delay, TickPriority::VeryHigh);
                        }
                    }
                }
            }
            BlockBehavior::Comparator => {
                let output = self.comparator_output(ctx.world, tick.pos, &state);
                let old_output = self.comparator_outputs.get(&tick.pos).copied().unwrap_or(0);
                self.cache_comparator_output(tick.pos, output);
                if old_output != output || state.property("mode") == Some("compare") {
                    let powered = output > 0;
                    if powered != state.bool_property("powered") {
                        let next = self.changed_state(state_id, "powered", powered.to_string())?;
                        self.set_diode_state(ctx, tick.pos, next, "comparator_tick")?;
                    }
                    self.update_diode_output_neighbors(ctx, tick.pos, &state);
                }
            }
            BlockBehavior::Observer => {
                let powered = !state.bool_property("powered");
                let next = self.changed_state(state_id, "powered", powered.to_string())?;
                self.set_diode_state(ctx, tick.pos, next, "observer_pulse")?;
                if powered {
                    ctx.schedule_tick(tick.pos, state.kind, 2, TickPriority::Normal);
                }
            }
            BlockBehavior::PressurePlate { .. }
            | BlockBehavior::Tripwire
            | BlockBehavior::DetectorRail => {
                self.refresh_entity_sensor(ctx, tick.pos, &state, true)?;
            }
            BlockBehavior::Lectern => {
                if state.bool_property("powered") {
                    let next = self.changed_state(state_id, "powered", "false")?;
                    if self.set_state_and_notify(ctx, tick.pos, next, "lectern_pulse_end", None)? {
                        ctx.update_neighbors(
                            tick.pos.relative(Direction::Down),
                            state.kind,
                            None,
                            None,
                        );
                    }
                }
            }
            BlockBehavior::Target => {
                if state.int_property("power").unwrap_or(0) != 0 {
                    let next = self.changed_state(state_id, "power", "0")?;
                    self.set_state_and_notify(ctx, tick.pos, next, "target_release", None)?;
                }
            }
            BlockBehavior::Lamp
                if state.bool_property("lit") && !self.is_powered(ctx.world, tick.pos) =>
            {
                let next = self.changed_state(state_id, "lit", "false")?;
                self.set_state_and_update_shapes(ctx, tick.pos, next, "lamp_off")?;
            }
            BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                self.execute_container_tick(ctx, tick.pos, &state)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn on_block_event(
        &mut self,
        ctx: &mut EventContext<'_>,
        event: BlockEvent,
    ) -> Result<bool, RulesError> {
        let state_id = ctx.world.get_block(event.pos);
        let state = self.state(state_id)?.clone();
        if state.kind != event.block || !matches!(state.behavior, BlockBehavior::Piston { .. }) {
            return Ok(false);
        }
        match self.move_piston(ctx, event.pos, state_id, event.param_a) {
            Ok(executed) => Ok(executed),
            Err(error) => {
                debug!(?error, pos = ?event.pos, "活塞事件未执行");
                Ok(false)
            }
        }
    }

    fn on_deferred_task(
        &mut self,
        ctx: &mut EventContext<'_>,
        task: DeferredRuleTask,
    ) -> Result<(), RulesError> {
        match task.kind {
            piston::CONTINUE_PISTON_RETRACTION => {
                let state_id = ctx.world.get_block(task.pos);
                let state = self.state(state_id)?;
                if matches!(state.behavior, BlockBehavior::Piston { .. }) {
                    self.continue_piston_retraction(ctx, task.pos, state_id, task.param_a)?;
                }
            }
            piston::SETTLE_MOVING_PISTON => {
                self.settle_moving_piston(ctx, task.pos, task.param_a != 0)?;
            }
            piston::MOVE_RETRACTED_STRUCTURE => {
                self.move_retracted_piston_structure(ctx, task.pos)?;
            }
            shape::UPDATE_NEIGHBOR_SHAPES => {
                self.update_neighbor_shapes(ctx, task.pos)?;
            }
            _ => {
                return Err(RulesError::Message(format!(
                    "unknown Java 26 deferred task: {}",
                    task.kind
                )));
            }
        }
        Ok(())
    }

    fn tick_entities(&mut self, ctx: &mut EventContext<'_>) -> Result<(), RulesError> {
        let item_entities = self.tick_minimal_entities(ctx)?;
        self.tick_hopper_entity_collisions(ctx, &item_entities)?;
        let mut occupied_positions = ctx
            .world
            .entities()
            .map(|(_, entity)| entity_block_pos(entity.position))
            .collect::<Vec<_>>();
        occupied_positions.sort_unstable();
        occupied_positions.dedup();
        for pos in occupied_positions {
            let state = self.state(ctx.world.get_block(pos))?.clone();
            if tracks_entity_collisions(&state.behavior) {
                self.refresh_entity_sensor(ctx, pos, &state, false)?;
            }
        }
        Ok(())
    }

    fn tick_block_entity(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
    ) -> Result<(), RulesError> {
        let state = self.state(ctx.world.get_block(pos))?.clone();
        if let Some(data) = ctx
            .world
            .block_entity(pos)
            .filter(|data| data.kind == "minecraft:moving_piston")
        {
            let should_settle = data
                .fields
                .get("settle_tick")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|tick| tick <= ctx.tick.0);
            if should_settle {
                self.settle_moving_piston(ctx, pos, false)?;
            }
            return Ok(());
        }
        match state.behavior {
            BlockBehavior::Hopper => self.tick_container(ctx, pos, &state)?,
            BlockBehavior::DaylightDetector => {
                if ctx.game_time() % 20 != 0 {
                    return Ok(());
                }
                let raw_sky_light = block_entity_i64(ctx.world, pos, "sky_signal")
                    .unwrap_or_else(|| i64::from(ctx.sky_light()))
                    .clamp(0, 15) as u8;
                let power = daylight_detector_power(
                    ctx.overworld_time(),
                    raw_sky_light,
                    state.bool_property("inverted"),
                );
                if state.int_property("power") != Some(i32::from(power)) {
                    let next = self.changed_state(state.id, "power", power.to_string())?;
                    self.set_state_and_notify(ctx, pos, next, "daylight_detector", None)?;
                }
            }
            BlockBehavior::TrappedChest => {
                let open = block_entity_i64(ctx.world, pos, "open_count")
                    .unwrap_or(0)
                    .clamp(0, 15);
                let previous = block_entity_i64(ctx.world, pos, "last_open_count").unwrap_or(0);
                if open != previous {
                    self.compiled.update_source_output(pos, open as u8);
                    ctx.update_block_entity(pos, |data| {
                        data.fields
                            .insert("last_open_count".to_owned(), serde_json::Value::from(open));
                    });
                    ctx.update_neighbors(pos, state.kind, None, None);
                    ctx.update_neighbors(pos.relative(Direction::Down), state.kind, None, None);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn should_tick_block_entity(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        data: &BlockEntityData,
    ) -> bool {
        if data.kind == "minecraft:moving_piston" {
            return true;
        }
        self.registry.state(world.get_block(pos)).is_some_and(|state| {
            matches!(
                state.behavior,
                BlockBehavior::Hopper
                    | BlockBehavior::DaylightDetector
                    | BlockBehavior::TrappedChest
            )
        })
    }

    fn read_probe(&self, world: &SparseWorld, probe: &Probe) -> ProbeValue {
        match probe {
            Probe::Signal { pos, direction } => {
                let signal = if let Some(direction) = direction {
                    self.signal(world, *pos, *direction)
                } else {
                    Direction::UPDATE_ORDER
                        .into_iter()
                        .map(|direction| self.signal(world, *pos, direction))
                        .max()
                        .unwrap_or(0)
                };
                ProbeValue::Integer(signal as i64)
            }
            Probe::BlockState { pos } => ProbeValue::State(world.get_block(*pos)),
            Probe::Property { pos, property } => self
                .registry
                .state(world.get_block(*pos))
                .and_then(|state| state.property(property))
                .map_or(ProbeValue::None, |value| {
                    ProbeValue::String(value.to_owned())
                }),
            Probe::ContainerCount { pos } => {
                ProbeValue::Integer(block_entity_i64(world, *pos, "item_count").unwrap_or(0))
            }
            Probe::EntityCount { kind } => ProbeValue::Integer(
                world
                    .entities()
                    .filter(|(_, entity)| kind.as_ref().is_none_or(|kind| &entity.kind == kind))
                    .count() as i64,
            ),
            Probe::EntityField { id, field } => world
                .entity(*id)
                .and_then(|entity| entity.fields.get(field))
                .map_or(ProbeValue::None, json_probe_value),
            Probe::EntityContainerCount { id } => {
                ProbeValue::Integer(world.entity(*id).map(entity_container_count).unwrap_or(0))
            }
            Probe::EventCount { kind } => {
                ProbeValue::Integer(self.event_counts.get(kind).copied().unwrap_or(0))
            }
        }
    }
}

impl Java26Rules {
    fn refresh_comparators_near(
        &mut self,
        ctx: &mut EventContext<'_>,
        changed: BlockPos,
    ) -> Result<(), RulesError> {
        for direction in Direction::HORIZONTAL {
            let direct = changed.relative(direction.opposite());
            let direct_state_id = ctx.world.get_block(direct);
            let direct_state = self.state(direct_state_id)?.clone();
            if matches!(direct_state.behavior, BlockBehavior::Comparator)
                && direct_state.facing == Some(direction)
            {
                self.refresh_comparator(ctx, direct, direct_state_id)?;
            }

            let conductor = changed.relative(direction.opposite());
            let conductor_state = self.state(ctx.world.get_block(conductor))?;
            if !conductor_state.redstone_conductor {
                continue;
            }
            let far = conductor.relative(direction.opposite());
            let far_state_id = ctx.world.get_block(far);
            let far_state = self.state(far_state_id)?.clone();
            if matches!(far_state.behavior, BlockBehavior::Comparator)
                && far_state.facing == Some(direction)
            {
                self.refresh_comparator(ctx, far, far_state_id)?;
            }
        }
        Ok(())
    }

    fn tick_minimal_entities(
        &mut self,
        ctx: &mut EventContext<'_>,
    ) -> Result<Vec<EntityId>, RulesError> {
        let mut expired = Vec::<EntityId>::new();
        let mut item_entities = Vec::<EntityId>::new();
        let entity_ids = ctx.world.entities().map(|(id, _)| *id).collect::<Vec<_>>();
        for id in entity_ids {
            let kind = ctx.world.entity(id).map(|entity| entity.kind.clone());
            match kind.as_deref() {
                Some("minecraft:item") => {
                    let Some(fields) = ctx.world.entity_fields_mut(id) else {
                        continue;
                    };
                    let age = fields
                        .get("age")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0)
                        + 1;
                    let pickup_delay = fields
                        .get("pickup_delay")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0);
                    let pickup_delay = if pickup_delay > 0 && pickup_delay != 32_767 {
                        pickup_delay.saturating_sub(1)
                    } else {
                        pickup_delay
                    };
                    fields.insert("age".to_owned(), serde_json::Value::from(age));
                    fields.insert(
                        "pickup_delay".to_owned(),
                        serde_json::Value::from(pickup_delay),
                    );
                    if age >= 6_000 {
                        expired.push(id);
                    } else {
                        item_entities.push(id);
                    }
                }
                Some("minecraft:tnt") => {
                    let Some(fields) = ctx.world.entity_fields_mut(id) else {
                        continue;
                    };
                    let fuse = fields
                        .get("fuse")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(80)
                        - 1;
                    fields.insert("fuse".to_owned(), serde_json::Value::from(fuse));
                    if fuse <= 0 {
                        expired.push(id);
                    }
                }
                Some("minecraft:hopper_minecart") => {
                    if !self.tick_hopper_minecart(ctx, id)? {
                        continue;
                    }
                    if let Some(pos) = ctx
                        .world
                        .entity(id)
                        .map(|entity| entity_block_pos(entity.position))
                    {
                        self.refresh_comparators_near(ctx, pos)?;
                    }
                    *self
                        .event_counts
                        .entry("hopper_minecart_transfer".to_owned())
                        .or_default() += 1;
                }
                _ => {}
            }
        }
        for id in expired {
            let kind = ctx.world.entity(id).map(|entity| entity.kind.clone());
            let pos = ctx
                .world
                .entity(id)
                .map(|entity| entity_block_pos(entity.position));
            ctx.world.remove_entity(id);
            if kind.as_deref() == Some("minecraft:tnt") {
                ctx.unsupported(pos.unwrap_or(BlockPos::ZERO), "tnt_explosion");
                *self
                    .event_counts
                    .entry("tnt_explosion".to_owned())
                    .or_default() += 1;
            } else {
                *self
                    .event_counts
                    .entry("item_despawn".to_owned())
                    .or_default() += 1;
            }
        }
        Ok(item_entities)
    }

    fn refresh_entity_sensor(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
        release_now: bool,
    ) -> Result<(), RulesError> {
        let (power, delay) = match state.behavior {
            BlockBehavior::PressurePlate {
                max_weight,
                detects_items,
            } => {
                let count = entities_on_block(ctx.world, pos, |entity| {
                    detects_items || entity.kind != "minecraft:item"
                });
                let power = max_weight.map_or_else(
                    || if count > 0 { 15 } else { 0 },
                    |max_weight| {
                        ((count as u64 * 15).div_ceil(u64::from(max_weight))).clamp(0, 15) as u8
                    },
                );
                (power, 20)
            }
            BlockBehavior::DetectorRail => {
                let count =
                    entities_on_block(ctx.world, pos, |entity| entity.kind.ends_with("minecart"));
                (if count > 0 { 15 } else { 0 }, 20)
            }
            BlockBehavior::Tripwire => {
                let count = entities_on_block(ctx.world, pos, |_| true);
                (if count > 0 { 15 } else { 0 }, 10)
            }
            _ => return Ok(()),
        };
        let current = state
            .int_property("power")
            .unwrap_or_else(|| i32::from(state.bool_property("powered")) * 15)
            .clamp(0, 15) as u8;
        if (power > 0 || release_now) && current != power {
            let property = if state.properties.contains_key("power") {
                "power"
            } else {
                "powered"
            };
            let value = if property == "power" {
                power.to_string()
            } else {
                (power > 0).to_string()
            };
            let next = self.changed_state(state.id, property, value)?;
            if self.set_state_and_notify(ctx, pos, next, "entity_sensor", None)?
                && matches!(
                    state.behavior,
                    BlockBehavior::PressurePlate { .. } | BlockBehavior::DetectorRail
                )
            {
                ctx.update_neighbors(pos.relative(Direction::Down), state.kind, None, None);
            }
            if matches!(state.behavior, BlockBehavior::Tripwire) {
                self.refresh_tripwire_hooks(ctx, pos, power > 0)?;
            }
        }
        if power > 0 && !ctx.has_scheduled_tick(pos, state.kind) {
            ctx.schedule_tick_after_neighbors(pos, state.kind, delay, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_tripwire_hooks(
        &mut self,
        ctx: &mut EventContext<'_>,
        wire_pos: BlockPos,
        powered: bool,
    ) -> Result<(), RulesError> {
        for direction in Direction::HORIZONTAL {
            let mut cursor = wire_pos.relative(direction);
            for _ in 0..42 {
                let state_id = ctx.world.get_block(cursor);
                let state = self.state(state_id)?.clone();
                match state.behavior {
                    BlockBehavior::Tripwire => cursor = cursor.relative(direction),
                    BlockBehavior::TripwireHook
                        if state.facing == Some(direction.opposite()) =>
                    {
                        let mut next = state_id;
                        if !state.bool_property("attached") {
                            next = self.changed_state(next, "attached", "true")?;
                        }
                        if state.bool_property("powered") != powered {
                            next = self.changed_state(next, "powered", powered.to_string())?;
                        }
                        if self.set_state_and_notify(ctx, cursor, next, "tripwire_hook", None)? {
                            update_attached_power_neighbors(ctx, cursor, &state);
                        }
                        break;
                    }
                    _ => break,
                }
            }
        }
        Ok(())
    }
}

fn item_frame_output(world: &SparseWorld, pos: BlockPos, facing: Direction) -> Option<u8> {
    let frames = world
        .entity_ids_in_aabb(
            [pos.x as f64, pos.y as f64, pos.z as f64],
            [pos.x as f64 + 1.0, pos.y as f64 + 1.0, pos.z as f64 + 1.0],
        )
        .into_iter()
        .filter_map(|id| world.entity(id))
        .filter(|entity| {
            matches!(
                entity.kind.as_str(),
                "minecraft:item_frame" | "minecraft:glow_item_frame"
            ) && entity_direction(entity) == Some(facing)
        })
        .collect::<Vec<_>>();
    if frames.len() != 1 {
        return None;
    }
    let frame = frames[0];
    let has_item = frame
        .fields
        .get("item_count")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0)
        > 0
        || frame
            .fields
            .get("item_id")
            .and_then(serde_json::Value::as_str)
            .is_some();
    if !has_item {
        return Some(0);
    }
    let rotation = frame
        .fields
        .get("rotation")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    Some(rotation.rem_euclid(8) as u8 + 1)
}

fn entity_direction(entity: &redstone_core::EntityData) -> Option<Direction> {
    match entity.fields.get("facing")? {
        serde_json::Value::String(value) => match value.as_str() {
            "west" => Some(Direction::West),
            "east" => Some(Direction::East),
            "down" => Some(Direction::Down),
            "up" => Some(Direction::Up),
            "north" => Some(Direction::North),
            "south" => Some(Direction::South),
            _ => None,
        },
        serde_json::Value::Number(value) => match value.as_i64()? {
            0 => Some(Direction::Down),
            1 => Some(Direction::Up),
            2 => Some(Direction::North),
            3 => Some(Direction::South),
            4 => Some(Direction::West),
            5 => Some(Direction::East),
            _ => None,
        },
        _ => None,
    }
}

fn entity_block_pos(position: [f64; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
        position[1].floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
        position[2].floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
    )
}

fn json_probe_value(value: &serde_json::Value) -> ProbeValue {
    match value {
        serde_json::Value::Bool(value) => ProbeValue::Bool(*value),
        serde_json::Value::Number(value) => {
            value.as_i64().map_or(ProbeValue::None, ProbeValue::Integer)
        }
        serde_json::Value::String(value) => ProbeValue::String(value.clone()),
        _ => ProbeValue::String(value.to_string()),
    }
}

fn entity_container_count(entity: &redstone_core::EntityData) -> i64 {
    entity
        .fields
        .get("inventory")
        .and_then(serde_json::Value::as_array)
        .map(|inventory| {
            inventory
                .iter()
                .filter_map(|entry| entry.get("count").and_then(serde_json::Value::as_i64))
                .sum()
        })
        .or_else(|| {
            entity
                .fields
                .get("item_count")
                .and_then(serde_json::Value::as_i64)
        })
        .unwrap_or(0)
}

fn tracks_entity_collisions(behavior: &BlockBehavior) -> bool {
    matches!(
        behavior,
        BlockBehavior::PressurePlate { .. } | BlockBehavior::Tripwire | BlockBehavior::DetectorRail
    )
}

fn entities_on_block(
    world: &SparseWorld,
    pos: BlockPos,
    predicate: impl Fn(&redstone_core::EntityData) -> bool,
) -> usize {
    world
        .entity_ids_in_aabb(
            [pos.x as f64, pos.y as f64, pos.z as f64],
            [pos.x as f64 + 1.0, pos.y as f64 + 1.0, pos.z as f64 + 1.0],
        )
        .into_iter()
        .filter(|id| world.entity(*id).is_some_and(&predicate))
        .count()
}

fn attached_block(pos: BlockPos, state: &StateDefinition) -> BlockPos {
    match state.behavior {
        BlockBehavior::Torch { wall: true } => {
            pos.relative(state.facing.unwrap_or(Direction::North).opposite())
        }
        _ => pos.relative(Direction::Down),
    }
}

fn wire_power_of(state: &StateDefinition) -> u8 {
    if matches!(state.behavior, BlockBehavior::Wire) {
        state.power
    } else {
        0
    }
}

#[cfg(test)]
mod diode_tests {
    use super::*;
    use crate::StateResolver;

    fn resolve_state(
        registry: &mut Java26Registry,
        name: &str,
        properties: &[(&str, &str)],
    ) -> BlockStateId {
        registry
            .resolve_state(
                name,
                &properties
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
            )
            .unwrap()
    }

    #[test]
    fn diode_input_reads_wire_power_without_a_facing_connection() {
        let mut registry = Java26Registry::new();
        let wire = resolve_state(
            &mut registry,
            "minecraft:redstone_wire",
            &[
                ("power", "7"),
                ("north", "none"),
                ("east", "none"),
                ("south", "none"),
                ("west", "none"),
            ],
        );
        let repeater = resolve_state(
            &mut registry,
            "minecraft:repeater",
            &[
                ("delay", "1"),
                ("facing", "north"),
                ("locked", "false"),
                ("powered", "false"),
            ],
        );
        let comparator = resolve_state(
            &mut registry,
            "minecraft:comparator",
            &[
                ("facing", "north"),
                ("mode", "compare"),
                ("powered", "false"),
            ],
        );
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(0, 0, -1), wire).unwrap();
        world.set_block(BlockPos::ZERO, repeater).unwrap();
        world.set_block(BlockPos::new(2, 0, -1), wire).unwrap();
        world.set_block(BlockPos::new(2, 0, 0), comparator).unwrap();
        let rules = Java26Rules::new(registry);

        assert_eq!(
            rules.signal(&world, BlockPos::new(0, 0, -1), Direction::North),
            0
        );
        assert_eq!(
            rules.diode_input(&world, BlockPos::ZERO, rules.state(repeater).unwrap()),
            7
        );
        assert_eq!(
            rules.comparator_output(
                &world,
                BlockPos::new(2, 0, 0),
                rules.state(comparator).unwrap(),
            ),
            7
        );
    }
}

fn power_value(power: u8) -> &'static str {
    const VALUES: [&str; 16] = [
        "0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13",
        "14", "15",
    ];
    VALUES[usize::from(power.min(15))]
}

fn attached_direction(state: &StateDefinition) -> Direction {
    match state.property("face") {
        Some("ceiling") => Direction::Down,
        Some("floor") => Direction::Up,
        _ => state.facing.unwrap_or(Direction::North),
    }
}

fn update_attached_power_neighbors(
    ctx: &mut EventContext<'_>,
    pos: BlockPos,
    state: &StateDefinition,
) {
    let attached = pos.relative(attached_direction(state).opposite());
    ctx.update_neighbors(pos, state.kind, None, None);
    ctx.update_neighbors(attached, state.kind, None, None);
}

fn update_torch_output_neighbors(
    ctx: &mut EventContext<'_>,
    pos: BlockPos,
    state: &StateDefinition,
) {
    let orientation = match ctx.mode {
        RedstoneMode::Default => None,
        RedstoneMode::Experimental => {
            let orientation = Orientation::from_index(ctx.random_bounded(48) as u8)
                .with_side_bias(SideBias::Left)
                .with_up(Direction::Up);
            Some(match state.behavior {
                BlockBehavior::Torch { wall: true } => orientation
                    .with_front(state.facing.unwrap_or(Direction::North).opposite()),
                _ => orientation,
            })
        }
    };
    for direction in [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ] {
        ctx.update_neighbors(
            pos.relative(direction),
            state.kind,
            None,
            orientation.map(|orientation| orientation.with_front(direction).index()),
        );
    }
}

fn torch_input_direction(state: &StateDefinition) -> Direction {
    match state.behavior {
        BlockBehavior::Torch { wall: true } => {
            state.facing.unwrap_or(Direction::North).opposite()
        }
        _ => Direction::Down,
    }
}

fn wire_connects_for_update(state: &StateDefinition, direction: Direction) -> bool {
    match direction {
        Direction::Down => true,
        Direction::Up => false,
        horizontal => state.property(direction_name(horizontal)) != Some("none"),
    }
}

fn default_wire_update_positions(pos: BlockPos) -> [BlockPos; 7] {
    const JAVA_DIRECTION_VALUES: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];
    let neighbors = JAVA_DIRECTION_VALUES.map(|direction| pos.relative(direction));
    let mut positions = [
        pos,
        neighbors[0],
        neighbors[1],
        neighbors[2],
        neighbors[3],
        neighbors[4],
        neighbors[5],
    ];
    positions.sort_by_key(|candidate| java_hash_map_bucket(*candidate));
    positions
}

fn java_hash_map_bucket(pos: BlockPos) -> u32 {
    let hash = pos
        .z
        .wrapping_mul(31)
        .wrapping_add(pos.y)
        .wrapping_mul(31)
        .wrapping_add(pos.x);
    let spread = hash ^ ((hash as u32 >> 16) as i32);
    spread as u32 & 15
}

fn oriented_wire_neighbors(
    world: &SparseWorld,
    pos: BlockPos,
    orientation: u8,
    registry: &Java26Registry,
) -> Vec<(BlockPos, u8)> {
    let orientation = Orientation::from_index(orientation);
    let mut result = Vec::new();
    for horizontal in orientation.horizontal_directions() {
        let candidate = pos.relative(horizontal);
        push_wire_candidate(
            &mut result,
            world,
            candidate,
            orientation.with_front(horizontal).index(),
            registry,
        );
    }
    for vertical in orientation.vertical_directions() {
        let offset = pos.relative(vertical);
        let offset_solid = registry
            .state(world.get_block(offset))
            .is_some_and(|state| state.redstone_conductor);
        for horizontal in orientation.horizontal_directions() {
            let side = pos.relative(horizontal);
            let side_solid = registry
                .state(world.get_block(side))
                .is_some_and(|state| state.redstone_conductor);
            let candidate = offset.relative(horizontal);
            if (vertical == Direction::Up && !offset_solid)
                || (vertical == Direction::Down && !side_solid)
            {
                push_wire_candidate(
                    &mut result,
                    world,
                    candidate,
                    orientation.with_front(horizontal).index(),
                    registry,
                );
            }
        }
    }
    result
}

fn push_wire_candidate(
    result: &mut Vec<(BlockPos, u8)>,
    world: &SparseWorld,
    pos: BlockPos,
    orientation: u8,
    registry: &Java26Registry,
) {
    if registry
        .state(world.get_block(pos))
        .is_some_and(|state| matches!(state.behavior, BlockBehavior::Wire))
        && !result.iter().any(|(candidate, _)| *candidate == pos)
    {
        result.push((pos, orientation));
    }
}

fn direction_index(direction: Direction) -> u8 {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StateResolver;

    #[test]
    fn default_wire_positions_follow_java_hash_set_bucket_order() {
        let pos = BlockPos::new(1, 1, 0);

        assert_eq!(
            default_wire_update_positions(pos),
            [
                pos,
                pos.relative(Direction::North),
                pos.relative(Direction::Down),
                pos.relative(Direction::South),
                pos.relative(Direction::East),
                pos.relative(Direction::Up),
                pos.relative(Direction::West),
            ]
        );
    }

    #[test]
    fn interpreted_cache_restore_uses_compiled_comparator_outputs() {
        let mut registry = Java26Registry::new();
        let properties = registry
            .complete_state_properties("minecraft:comparator", &BTreeMap::new())
            .unwrap();
        let comparator = registry
            .resolve_state("minecraft:comparator", &properties)
            .unwrap();
        let comparator_pos = BlockPos::ZERO;
        let stale_pos = BlockPos::new(1, 0, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(comparator_pos, comparator).unwrap();
        let mut rules = Java26Rules::new(registry);
        rules.block_signal_cache.insert(BlockPos::new(2, 0, 0), 15);
        rules.comparator_outputs.insert(stale_pos, 3);
        rules.compiled.update_comparator_output(comparator_pos, 11);
        rules.compiled.update_comparator_output(stale_pos, 7);

        rules.restore_interpreted_caches(&world);

        assert!(rules.block_signal_cache.is_empty());
        assert_eq!(
            rules.comparator_outputs,
            BTreeMap::from([(comparator_pos, 11)])
        );
        assert_eq!(
            rules.compiled.comparator_outputs_snapshot(),
            rules.comparator_outputs
        );
    }
}
