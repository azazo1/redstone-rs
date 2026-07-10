use std::collections::{BTreeMap, BTreeSet, VecDeque};

use indexmap::IndexSet;
use redstone_core::{
    Action, BlockEvent, BlockKindId, BlockPos, BlockRules, BlockStateId, DeferredRuleTask,
    Direction, EntityId, EventContext, NeighborUpdate, Probe, ProbeValue, RedstoneMode, RulesError,
    ScheduledTick, SparseWorld, TickPriority,
};
use tracing::debug;

use crate::{BlockBehavior, JAVA_VERSION, Java26Registry, PushReaction, StateDefinition};
use crate::orientation::{Orientation, SideBias};

mod inventory;
mod piston;
mod shape;
mod consumer;

use inventory::{block_entity_i64, container_signal};

pub struct Java26Rules {
    registry: Java26Registry,
    comparator_outputs: BTreeMap<BlockPos, u8>,
    torch_toggles: VecDeque<(u64, BlockPos)>,
    event_counts: BTreeMap<String, i64>,
    entity_sensors: BTreeSet<BlockPos>,
}

impl Java26Rules {
    pub fn new(registry: Java26Registry) -> Self {
        Self {
            registry,
            comparator_outputs: BTreeMap::new(),
            torch_toggles: VecDeque::new(),
            event_counts: BTreeMap::new(),
            entity_sensors: BTreeSet::new(),
        }
    }

    pub fn registry(&self) -> &Java26Registry {
        &self.registry
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
        value: impl Into<String>,
    ) -> Result<BlockStateId, RulesError> {
        self.registry
            .with_property(state, property, value)
            .map_err(|error| RulesError::Message(error.to_string()))
    }

    fn set_state_and_notify(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: BlockStateId,
        cause: &str,
        orientation: Option<u8>,
    ) -> Result<bool, RulesError> {
        let old = ctx.set_block(pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        self.sync_entity_sensor(pos, old, state);
        let source_block = self.registry.state(state).map_or_else(
            || self.registry.state(old).map(|state| state.kind),
            |state| Some(state.kind),
        );
        if let Some(source_block) = source_block {
            if let Some(orientation) = orientation {
                let orientation = Orientation::from_index(orientation);
                let changed_state = self.state(state)?.clone();
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
                                orientation: Some(
                                    orientation.with_front(direction).index(),
                                ),
                                moved_by_piston: false,
                            });
                        }
                    }
                }
            }
        }
        Ok(true)
    }

    fn update_diode_output_neighbors(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) {
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
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
        let old = ctx.set_block(pos, state, cause)?;
        if old == state {
            return Ok(false);
        }
        self.sync_entity_sensor(pos, old, state);
        let definition = self.state(state)?.clone();
        self.update_diode_output_neighbors(ctx, pos, &definition);
        Ok(true)
    }

    fn sync_entity_sensor(
        &mut self,
        pos: BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
    ) {
        if self
            .registry
            .state(old_state)
            .is_some_and(|state| tracks_entity_collisions(&state.behavior))
        {
            self.entity_sensors.remove(&pos);
        }
        if self
            .registry
            .state(new_state)
            .is_some_and(|state| tracks_entity_collisions(&state.behavior))
        {
            self.entity_sensors.insert(pos);
        }
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

    fn signal(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> u8 {
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
                self.direct_signal(
                    world,
                    pos.relative(neighbor_direction),
                    neighbor_direction,
                )
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
                let power = state.int_property("power").unwrap_or(0).clamp(0, 15) as u8;
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
                if state.bool_property("powered") { 15 } else { 0 }
            }
            BlockBehavior::Torch { wall } => {
                if !state.bool_property("lit") {
                    return 0;
                }
                if (*wall && state.direction_property("facing") == Some(direction))
                    || (!*wall && direction == Direction::Up)
                {
                    0
                } else {
                    15
                }
            }
            BlockBehavior::Repeater => {
                if state.bool_property("powered")
                    && state.direction_property("facing") == Some(direction)
                {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::Comparator => {
                if state.direction_property("facing") == Some(direction) {
                    self.comparator_outputs.get(&pos).copied().unwrap_or(0)
                } else {
                    0
                }
            }
            BlockBehavior::Observer => {
                if state.bool_property("powered")
                    && state.direction_property("facing") == Some(direction)
                {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::Target => state
                .int_property("power")
                .unwrap_or(0)
                .clamp(0, 15) as u8,
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

    fn direct_signal(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        direction: Direction,
    ) -> u8 {
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
            BlockBehavior::DetectorRail
            | BlockBehavior::Lectern
            | BlockBehavior::TrappedChest => {
                if direction == Direction::Up {
                    self.weak_signal(world, pos, state, direction)
                } else {
                    0
                }
            }
            BlockBehavior::TripwireHook => {
                if state.direction_property("facing") == Some(direction) {
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
        Direction::UPDATE_ORDER
            .into_iter()
            .map(|neighbor_direction| {
                let neighbor_pos = pos.relative(neighbor_direction);
                let Ok(neighbor) = self.state(world.get_block(neighbor_pos)) else {
                    return 0;
                };
                if matches!(neighbor.behavior, BlockBehavior::Wire) {
                    0
                } else {
                    self.direct_signal(world, neighbor_pos, neighbor_direction)
                }
            })
            .max()
            .unwrap_or(0)
            .max(own_signal)
    }

    fn wire_target_power(&self, world: &SparseWorld, pos: BlockPos) -> u8 {
        let mut block_power = 0u8;
        for direction in Direction::UPDATE_ORDER {
            let neighbor = pos.relative(direction);
            block_power = block_power.max(self.signal_without_wire_feedback(
                world,
                neighbor,
                direction,
            ));
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
        }
        block_power.max(wire_power.saturating_sub(1))
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
                let old_power = state.int_property("power").unwrap_or(0).clamp(0, 15) as u8;
                let new_power = self.wire_target_power(ctx.world, initial_pos);
                if old_power == new_power {
                    return Ok(());
                }
                let new_state = self.changed_state(state_id, "power", new_power.to_string())?;
                let old_state = ctx.set_block(initial_pos, new_state, "default_wire")?;
                self.sync_entity_sensor(initial_pos, old_state, new_state);
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
                    let old = state.int_property("power").unwrap_or(0).clamp(0, 15) as u8;
                    let target = self.wire_target_power(ctx.world, pos);
                    let next = if target < old { 0 } else { target };
                    if target > 0 && target < old {
                        turn_on.push_back((pos, orientation));
                    }
                    if next != old {
                        let new_state = self.changed_state(state_id, "power", next.to_string())?;
                        self.set_state_and_notify(
                            ctx,
                            pos,
                            new_state,
                            "experimental_wire_off",
                            Some(orientation),
                        )?;
                    }
                    for (candidate, candidate_orientation) in oriented_wire_neighbors(
                        ctx.world,
                        pos,
                        orientation,
                        &self.registry,
                    ) {
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
                    let old = state.int_property("power").unwrap_or(0).clamp(0, 15) as u8;
                    let target = self.wire_target_power(ctx.world, pos);
                    if target > old {
                        let new_state = self.changed_state(state_id, "power", target.to_string())?;
                        self.set_state_and_notify(
                            ctx,
                            pos,
                            new_state,
                            "experimental_wire_on",
                            Some(orientation),
                        )?;
                        for (candidate, candidate_orientation) in oriented_wire_neighbors(
                            ctx.world,
                            pos,
                            orientation,
                            &self.registry,
                        ) {
                            turn_on.push_back((candidate, candidate_orientation));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn refresh_torch(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let attached = attached_block(pos, &state);
        let should_be_lit = self.signal(ctx.world, attached, torch_input_direction(&state)) == 0;
        if state.bool_property("lit") != should_be_lit
            && !ctx.has_scheduled_tick(pos, state.kind)
        {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_repeater(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        if self.repeater_locked(ctx.world, pos, &state) {
            return Ok(());
        }
        let should_power = self.diode_input(ctx.world, pos, &state) > 0;
        if should_power != state.bool_property("powered")
            && !ctx.has_scheduled_tick(pos, state.kind)
        {
            let priority = if state.bool_property("powered") {
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
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let rear = pos.relative(facing);
        self.signal(world, rear, facing)
    }

    fn side_input(&self, world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        [facing.clockwise(), facing.counter_clockwise()]
            .into_iter()
            .map(|direction| {
                let source = pos.relative(direction);
                self.signal(world, source, direction)
            })
            .max()
            .unwrap_or(0)
    }

    fn repeater_locked(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> bool {
        self.side_input(world, pos, state) > 0
    }

    fn comparator_output(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> u8 {
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

    fn analog_input(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> u8 {
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let rear = pos.relative(facing);
        if let Some(value) = container_signal(world, rear) {
            return value;
        }
        if let Some(value) = block_entity_i64(world, rear, "comparator_output") {
            return value.clamp(0, 15) as u8;
        }
        let direct = self.signal(world, rear, facing);
        let rear_state = self.state(world.get_block(rear)).ok();
        if direct < 15 && rear_state.is_some_and(|state| state.redstone_conductor) {
            let far = rear.relative(facing);
            let far_output = container_signal(world, far)
                .or_else(|| block_entity_i64(world, far, "comparator_output").map(|value| value.clamp(0, 15) as u8))
                .or_else(|| item_frame_output(world, far, facing));
            if let Some(output) = far_output {
                return output;
            }
        }
        direct
    }

    fn refresh_comparator(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let output = self.comparator_output(ctx.world, pos, &state);
        let old = self.comparator_outputs.get(&pos).copied().unwrap_or(0);
        let should_power = output > 0;
        if (old != output || state.bool_property("powered") != should_power)
            && !ctx.has_scheduled_tick(pos, state.kind)
        {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_observer(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        source_pos: BlockPos,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::South);
        if pos.relative(facing) == source_pos
            && !state.bool_property("powered")
            && !ctx.has_scheduled_tick(pos, state.kind)
        {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }

    fn refresh_triggered_container(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = self.is_powered(ctx.world, pos)
            || matches!(state.behavior, BlockBehavior::Dropper | BlockBehavior::Dispenser)
                && self.is_powered(ctx.world, pos.relative(Direction::Up));
        let triggered = state.bool_property("triggered");
        if powered && !triggered {
            let next = self.changed_state(state.id, "triggered", "true")?;
            ctx.set_block(pos, next, "container_trigger")?;
            ctx.schedule_tick(pos, state.kind, 4, TickPriority::Normal);
        } else if !powered && triggered {
            let next = self.changed_state(state.id, "triggered", "false")?;
            ctx.set_block(pos, next, "container_untrigger")?;
        }
        Ok(())
    }

    fn refresh_powered_consumer(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = self.is_powered(ctx.world, pos);
        match state.behavior {
            BlockBehavior::Lamp => {
                if powered && !state.bool_property("lit") {
                    let next = self.changed_state(state_id, "lit", "true")?;
                    self.set_state_and_notify(ctx, pos, next, "lamp_on", None)?;
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
                if state.properties.contains_key("open")
                    && state.bool_property("open") != powered
                {
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
                self.set_state_and_notify(
                    ctx,
                    pos,
                    self.registry.air_state(),
                    "tnt_primed",
                    None,
                )?;
                ctx.unsupported(pos, "tnt_explosion");
                *self.event_counts.entry("tnt_primed".to_owned()).or_default() += 1;
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
                    if self.set_state_and_notify(
                        ctx,
                        pos,
                        next,
                        "lectern_page_change",
                        None,
                    )? {
                        ctx.update_neighbors(
                            pos.relative(Direction::Down),
                            state.kind,
                            None,
                            None,
                        );
                    }
                    ctx.schedule_tick_after_neighbors(
                        pos,
                        state.kind,
                        2,
                        TickPriority::Normal,
                    );
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

impl BlockRules for Java26Rules {
    fn version(&self) -> &str {
        JAVA_VERSION
    }

    fn air_state(&self) -> BlockStateId {
        self.registry.air_state()
    }

    fn block_kind(&self, state: BlockStateId) -> BlockKindId {
        self.registry.state(state).map_or(BlockKindId(0), |state| state.kind)
    }

    fn block_name(&self, state: BlockStateId) -> &str {
        self.registry
            .state(state)
            .map_or("minecraft:unknown", |state| state.name.as_str())
    }

    fn is_supported(&self, state: BlockStateId) -> bool {
        self.registry.state(state).is_some_and(|state| state.supported)
    }

    fn load_world(&mut self, world: &SparseWorld) -> Result<(), RulesError> {
        self.entity_sensors.clear();
        self.entity_sensors.extend(world.iter_blocks().filter_map(|(pos, state)| {
            self.registry
                .state(state)
                .is_some_and(|state| tracks_entity_collisions(&state.behavior))
                .then_some(pos)
        }));
        Ok(())
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
            self.sync_entity_sensor(*pos, self.registry.air_state(), state_id);
            match state.behavior {
                BlockBehavior::Wire => self.update_wire(ctx, *pos, None)?,
                BlockBehavior::Torch { .. } => self.refresh_torch(ctx, *pos, state_id)?,
                BlockBehavior::Repeater => self.refresh_repeater(ctx, *pos, state_id)?,
                BlockBehavior::Comparator => self.refresh_comparator(ctx, *pos, state_id)?,
                BlockBehavior::Piston { .. } => self.refresh_piston(ctx, *pos, state_id)?,
                BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                    self.refresh_triggered_container(ctx, *pos, state_id)?
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

    fn apply_action(
        &mut self,
        ctx: &mut EventContext<'_>,
        action: &Action,
    ) -> Result<(), RulesError> {
        match action {
            Action::SetBlock { pos, state } => {
                self.set_state_and_notify(ctx, *pos, *state, "action_set_block", None)?;
                self.repair_shape(ctx, *pos, true)?;
                ctx.run_rule_task_after_neighbors(DeferredRuleTask {
                    kind: shape::REPAIR_NEIGHBOR_SHAPES,
                    pos: *pos,
                    param_a: 0,
                    param_b: 0,
                });
            }
            Action::BreakBlock { pos } => {
                self.set_state_and_notify(
                    ctx,
                    *pos,
                    self.registry.air_state(),
                    "action_break_block",
                    None,
                )?;
                ctx.run_rule_task_after_neighbors(DeferredRuleTask {
                    kind: shape::REPAIR_NEIGHBOR_SHAPES,
                    pos: *pos,
                    param_a: 0,
                    param_b: 0,
                });
            }
            Action::UseBlock { pos } => self.use_block(ctx, *pos, false, false)?,
            Action::PressButton { pos } => self.use_block(ctx, *pos, true, false)?,
            Action::PullLever { pos } => self.use_block(ctx, *pos, false, true)?,
            Action::SetBlockEntity { pos, data } => {
                ctx.world.set_block_entity(*pos, data.clone());
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
    ) -> Result<(), RulesError> {
        let current_state_id = ctx.world.get_block(update.pos);
        let current_state = self.state(current_state_id)?;
        let state_id = if matches!(current_state.behavior, BlockBehavior::Wire) {
            current_state_id
        } else {
            self.repair_shape(ctx, update.pos, true)?
        };
        let state = self.state(state_id)?.clone();
        if state.name == "minecraft:piston_head" {
            self.refresh_piston_head(ctx, update.pos, &state, update)?;
        }
        match state.behavior {
            BlockBehavior::Wire => self.update_wire(ctx, update.pos, update.orientation)?,
            BlockBehavior::Torch { .. } => self.refresh_torch(ctx, update.pos, state_id)?,
            BlockBehavior::Repeater => self.refresh_repeater(ctx, update.pos, state_id)?,
            BlockBehavior::Comparator => self.refresh_comparator(ctx, update.pos, state_id)?,
            BlockBehavior::Observer => {
                self.refresh_observer(ctx, update.pos, state_id, update.source_pos)?
            }
            BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                self.refresh_triggered_container(ctx, update.pos, state_id)?
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
                        *self.event_counts.entry("torch_burnout".to_owned()).or_default() += 1;
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
                            let delay = state
                                .int_property("delay")
                                .unwrap_or(1)
                                .clamp(1, 4) as u64
                                * 2;
                            ctx.schedule_tick(
                                tick.pos,
                                state.kind,
                                delay,
                                TickPriority::VeryHigh,
                            );
                        }
                    }
                }
            }
            BlockBehavior::Comparator => {
                let output = self.comparator_output(ctx.world, tick.pos, &state);
                let old_output = self.comparator_outputs.get(&tick.pos).copied().unwrap_or(0);
                self.comparator_outputs.insert(tick.pos, output);
                if old_output != output || state.property("mode") == Some("compare") {
                    let powered = output > 0;
                    if powered != state.bool_property("powered") {
                        let next =
                            self.changed_state(state_id, "powered", powered.to_string())?;
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
                    if self.set_state_and_notify(
                        ctx,
                        tick.pos,
                        next,
                        "lectern_pulse_end",
                        None,
                    )? {
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
                self.set_state_and_notify(ctx, tick.pos, next, "lamp_off", None)?;
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
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(event.pos);
        let state = self.state(state_id)?.clone();
        if state.kind == event.block
            && matches!(state.behavior, BlockBehavior::Piston { .. })
            && let Err(error) = self.move_piston(ctx, event.pos, state_id, event.param_a)
        {
            debug!(?error, pos = ?event.pos, "活塞事件未执行");
        }
        Ok(())
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
            shape::REPAIR_NEIGHBOR_SHAPES => {
                self.repair_neighbor_shapes(ctx, task.pos)?;
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
        self.tick_minimal_entities(ctx);
        let sensors = self.entity_sensors.iter().copied().collect::<Vec<_>>();
        for pos in sensors {
            let state = self.state(ctx.world.get_block(pos))?.clone();
            if tracks_entity_collisions(&state.behavior) {
                self.refresh_entity_sensor(ctx, pos, &state, false)?;
            } else {
                self.entity_sensors.remove(&pos);
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
                let sky = block_entity_i64(ctx.world, pos, "sky_signal")
                    .unwrap_or_else(|| state.int_property("power").unwrap_or(0).into())
                    .clamp(0, 15);
                let power = if state.bool_property("inverted") { 15 - sky } else { sky };
                if state.int_property("power") != Some(power as i32) {
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
                    if let Some(data) = ctx.world.block_entity_mut(pos) {
                        data.fields.insert(
                            "last_open_count".to_owned(),
                            serde_json::Value::from(open),
                        );
                    }
                    ctx.update_neighbors(pos, state.kind, None, None);
                }
            }
            _ => {}
        }
        Ok(())
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
                .map_or(ProbeValue::None, |value| ProbeValue::String(value.to_owned())),
            Probe::ContainerCount { pos } => ProbeValue::Integer(
                block_entity_i64(world, *pos, "item_count").unwrap_or(0),
            ),
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
            Probe::EntityContainerCount { id } => ProbeValue::Integer(
                world
                    .entity(*id)
                    .map(entity_container_count)
                    .unwrap_or(0),
            ),
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
            for distance in 1..=2 {
                let pos = BlockPos::new(
                    changed.x - direction.step().0 * distance,
                    changed.y,
                    changed.z - direction.step().2 * distance,
                );
                let state_id = ctx.world.get_block(pos);
                let state = self.state(state_id)?.clone();
                if matches!(state.behavior, BlockBehavior::Comparator)
                    && state.direction_property("facing") == Some(direction)
                {
                    self.refresh_comparator(ctx, pos, state_id)?;
                }
            }
        }
        Ok(())
    }

    fn tick_minimal_entities(&mut self, ctx: &mut EventContext<'_>) {
        let mut expired = Vec::<EntityId>::new();
        let entity_ids = ctx.world.entities().map(|(id, _)| *id).collect::<Vec<_>>();
        for id in entity_ids {
            let kind = ctx
                .world
                .entity(id)
                .map(|entity| entity.kind.clone());
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
                Some("minecraft:hopper_minecart")
                    if minecart_enabled(ctx.world, id)
                        && absorb_into_hopper_minecart(ctx.world, id) =>
                {
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
                *self.event_counts.entry("tnt_explosion".to_owned()).or_default() += 1;
            } else {
                *self.event_counts.entry("item_despawn".to_owned()).or_default() += 1;
            }
        }
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
                let count = entities_on_block(ctx.world, pos, |entity| {
                    entity.kind.ends_with("minecart")
                });
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
                ctx.update_neighbors(
                    pos.relative(Direction::Down),
                    state.kind,
                    None,
                    None,
                );
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
                        if state.direction_property("facing") == Some(direction.opposite()) =>
                    {
                        let mut next = state_id;
                        if !state.bool_property("attached") {
                            next = self.changed_state(next, "attached", "true")?;
                        }
                        if state.bool_property("powered") != powered {
                            next = self.changed_state(next, "powered", powered.to_string())?;
                        }
                        if self.set_state_and_notify(
                            ctx,
                            cursor,
                            next,
                            "tripwire_hook",
                            None,
                        )? {
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
        serde_json::Value::Number(value) => value
            .as_i64()
            .map_or(ProbeValue::None, ProbeValue::Integer),
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
        .or_else(|| entity.fields.get("item_count").and_then(serde_json::Value::as_i64))
        .unwrap_or(0)
}

fn minecart_enabled(world: &SparseWorld, id: EntityId) -> bool {
    world
        .entity(id)
        .and_then(|entity| entity.fields.get("enabled"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true)
}

fn absorb_into_hopper_minecart(world: &mut SparseWorld, id: EntityId) -> bool {
    let Some(position) = world.entity(id).map(|entity| entity.position) else {
        return false;
    };
    let item = world
        .entity_ids_in_aabb(
            [position[0] - 0.75, position[1] - 0.25, position[2] - 0.75],
            [position[0] + 0.75, position[1] + 1.25, position[2] + 0.75],
        )
        .into_iter()
        .filter(|candidate| *candidate != id)
        .find_map(|candidate| {
            let entity = world.entity(candidate)?;
            (entity.kind == "minecraft:item").then(|| {
                (
                    candidate,
                    entity
                        .fields
                        .get("item_id")
                        .and_then(serde_json::Value::as_str)
                        .map(ToOwned::to_owned),
                    entity
                        .fields
                        .get("item_count")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(1),
                )
            })
        });
    let Some((item_id, Some(item_name), item_count)) = item else {
        return false;
    };
    if !insert_entity_item(world, id, &item_name) {
        return false;
    }
    if item_count <= 1 {
        world.remove_entity(item_id);
    } else if let Some(fields) = world.entity_fields_mut(item_id) {
        fields.insert("item_count".to_owned(), serde_json::Value::from(item_count - 1));
    }
    true
}

fn insert_entity_item(world: &mut SparseWorld, id: EntityId, item_id: &str) -> bool {
    let Some(entity) = world.entity(id) else {
        return false;
    };
    let mut inventory = entity
        .fields
        .get("inventory")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let capacity = entity
        .fields
        .get("capacity")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(5 * 64);
    if entity_container_count(entity) >= capacity {
        return false;
    }
    if let Some(stack) = inventory.iter_mut().find(|stack| {
        stack.get("item_id").and_then(serde_json::Value::as_str) == Some(item_id)
            && stack
                .get("count")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
                < 64
    }) {
        let count = stack
            .get("count")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            + 1;
        stack["count"] = serde_json::Value::from(count);
    } else {
        let used = inventory
            .iter()
            .filter_map(|stack| stack.get("slot").and_then(serde_json::Value::as_u64))
            .collect::<BTreeSet<_>>();
        let Some(slot) = (0..5u64).find(|slot| !used.contains(slot)) else {
            return false;
        };
        inventory.push(serde_json::json!({
            "slot": slot,
            "item_id": item_id,
            "count": 1,
        }));
        inventory.sort_by_key(|stack| {
            stack
                .get("slot")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX)
        });
    }
    let count = inventory
        .iter()
        .filter_map(|stack| stack.get("count").and_then(serde_json::Value::as_i64))
        .sum::<i64>();
    if let Some(fields) = world.entity_fields_mut(id) {
        fields.insert("inventory".to_owned(), serde_json::Value::Array(inventory));
        fields.insert("item_count".to_owned(), serde_json::Value::from(count));
        fields.entry("capacity".to_owned()).or_insert_with(|| serde_json::Value::from(320));
    }
    true
}

fn tracks_entity_collisions(behavior: &BlockBehavior) -> bool {
    matches!(
        behavior,
        BlockBehavior::PressurePlate { .. }
            | BlockBehavior::Tripwire
            | BlockBehavior::DetectorRail
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
        BlockBehavior::Torch { wall: true } => pos.relative(
            state
                .direction_property("facing")
                .unwrap_or(Direction::North)
                .opposite(),
        ),
        _ => pos.relative(Direction::Down),
    }
}

fn wire_power_of(state: &StateDefinition) -> u8 {
    if matches!(state.behavior, BlockBehavior::Wire) {
        state.int_property("power").unwrap_or(0).clamp(0, 15) as u8
    } else {
        0
    }
}

fn attached_direction(state: &StateDefinition) -> Direction {
    match state.property("face") {
        Some("ceiling") => Direction::Down,
        Some("floor") => Direction::Up,
        _ => state.direction_property("facing").unwrap_or(Direction::North),
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

fn torch_input_direction(state: &StateDefinition) -> Direction {
    match state.behavior {
        BlockBehavior::Torch { wall: true } => state
            .direction_property("facing")
            .unwrap_or(Direction::North)
            .opposite(),
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

fn default_wire_update_positions(pos: BlockPos) -> Vec<BlockPos> {
    const JAVA_DIRECTION_VALUES: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];
    let mut positions = Vec::with_capacity(7);
    positions.push(pos);
    positions.extend(JAVA_DIRECTION_VALUES.map(|direction| pos.relative(direction)));
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
}
