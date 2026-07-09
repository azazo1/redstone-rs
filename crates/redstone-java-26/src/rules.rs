use std::collections::{BTreeMap, BTreeSet, VecDeque};

use indexmap::IndexSet;
use redstone_core::{
    Action, BlockEvent, BlockKindId, BlockPos, BlockRules, BlockStateId, Direction, EntityId,
    EventContext, NeighborUpdate, Probe, ProbeValue, RedstoneMode, RulesError, ScheduledTick,
    SparseWorld, TickPriority,
};
use tracing::debug;

use crate::{BlockBehavior, JAVA_VERSION, Java26Registry, PushReaction, StateDefinition};
use crate::orientation::{Orientation, SideBias};

mod inventory;
mod piston;

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
                for direction in orientation.directions() {
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
                ctx.update_neighbors(pos, source_block, None, None);
            }
        }
        Ok(true)
    }

    fn set_state_and_notify_piston(
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
        let source_block = self
            .registry
            .state(state)
            .or_else(|| self.registry.state(old))
            .map(|definition| definition.kind);
        if let Some(source_block) = source_block {
            for direction in Direction::UPDATE_ORDER {
                ctx.neighbor_changed(NeighborUpdate {
                    pos: pos.relative(direction),
                    source_pos: pos,
                    source_block,
                    orientation: None,
                    moved_by_piston: true,
                });
            }
        }
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
            BlockBehavior::CopperBulb if state.bool_property("lit") => 15,
            BlockBehavior::CopperBulb => 0,
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
            BlockBehavior::Torch { wall: false } if direction != Direction::Down => 0,
            BlockBehavior::Lever | BlockBehavior::Button { .. } => {
                if state.bool_property("powered") && attached_direction(state) == direction {
                    15
                } else {
                    0
                }
            }
            BlockBehavior::PressurePlate { .. } => {
                if direction == Direction::Down {
                    self.weak_signal(world, pos, state, direction)
                } else {
                    0
                }
            }
            _ => self.weak_signal(world, pos, state, direction),
        }
    }

    fn wire_target_power(&self, world: &SparseWorld, pos: BlockPos) -> u8 {
        let mut block_power = 0u8;
        let mut wire_power = 0u8;
        for direction in Direction::UPDATE_ORDER {
            let neighbor = pos.relative(direction);
            let neighbor_state = world.get_block(neighbor);
            let Ok(definition) = self.state(neighbor_state) else {
                continue;
            };
            if matches!(definition.behavior, BlockBehavior::Wire) {
                wire_power = wire_power.max(
                    definition
                        .int_property("power")
                        .unwrap_or(0)
                        .clamp(0, 15) as u8,
                );
            } else {
                block_power = block_power.max(self.signal(
                    world,
                    neighbor,
                    direction,
                ));
            }
        }

        for horizontal in Direction::HORIZONTAL {
            let side = pos.relative(horizontal);
            let Ok(side_state) = self.state(world.get_block(side)) else {
                continue;
            };
            let candidate = if side_state.redstone_conductor {
                side.relative(Direction::Up)
            } else {
                side.relative(Direction::Down)
            };
            if let Ok(candidate_state) = self.state(world.get_block(candidate))
                && matches!(candidate_state.behavior, BlockBehavior::Wire)
            {
                wire_power = wire_power.max(
                    candidate_state
                        .int_property("power")
                        .unwrap_or(0)
                        .clamp(0, 15) as u8,
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
                let mut queue = VecDeque::from([initial_pos]);
                let mut queued = BTreeSet::from([initial_pos]);
                while let Some(pos) = queue.pop_front() {
                    queued.remove(&pos);
                    let state_id = ctx.world.get_block(pos);
                    let state = self.state(state_id)?.clone();
                    if !matches!(state.behavior, BlockBehavior::Wire) {
                        continue;
                    }
                    let old_power = state.int_property("power").unwrap_or(0).clamp(0, 15) as u8;
                    let new_power = self.wire_target_power(ctx.world, pos);
                    if old_power == new_power {
                        continue;
                    }
                    let new_state = self.changed_state(state_id, "power", new_power.to_string())?;
                    self.set_state_and_notify(ctx, pos, new_state, "default_wire", None)?;
                    for candidate in wire_neighbors(ctx.world, pos, &self.registry) {
                        if queued.insert(candidate) {
                            queue.push_back(candidate);
                        }
                    }
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
        self.signal(world, rear, facing)
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
                self.set_state_and_notify(ctx, pos, next, "lever_use", None)?;
            }
            BlockBehavior::Button { wooden } if !force_lever => {
                if !state.bool_property("powered") {
                    let next = self.changed_state(state_id, "powered", "true")?;
                    self.set_state_and_notify(ctx, pos, next, "button_press", None)?;
                    ctx.schedule_tick(
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
                    self.set_state_and_notify(ctx, pos, next, "lectern_page_change", None)?;
                    ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
                }
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
            let state_id = ctx.world.get_block(*pos);
            let state = self.state(state_id)?.clone();
            self.sync_entity_sensor(*pos, self.registry.air_state(), state_id);
            match state.behavior {
                BlockBehavior::Wire => self.update_wire(ctx, *pos, None)?,
                BlockBehavior::Torch { .. } => self.refresh_torch(ctx, *pos, state_id)?,
                BlockBehavior::Repeater => self.refresh_repeater(ctx, *pos, state_id)?,
                BlockBehavior::Comparator => self.refresh_comparator(ctx, *pos, state_id)?,
                BlockBehavior::Piston { .. } => self.refresh_piston(ctx, *pos, state_id)?,
                BlockBehavior::Lamp
                | BlockBehavior::CopperBulb
                | BlockBehavior::PoweredConsumer
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
                ctx.world.set_block_entity(*pos, data.clone());
                if let Ok(state) = self.state(ctx.world.get_block(*pos))
                    && matches!(state.behavior, BlockBehavior::Comparator)
                {
                    self.refresh_comparator(ctx, *pos, state.id)?;
                }
            }
        }
        Ok(())
    }

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(update.pos);
        let state = self.state(state_id)?.clone();
        match state.behavior {
            BlockBehavior::Wire => self.update_wire(ctx, update.pos, update.orientation)?,
            BlockBehavior::Torch { .. } => self.refresh_torch(ctx, update.pos, state_id)?,
            BlockBehavior::Repeater => self.refresh_repeater(ctx, update.pos, state_id)?,
            BlockBehavior::Comparator => self.refresh_comparator(ctx, update.pos, state_id)?,
            BlockBehavior::Observer => {
                self.refresh_observer(ctx, update.pos, state_id, update.source_pos)?
            }
            BlockBehavior::Piston { .. } => self.refresh_piston(ctx, update.pos, state_id)?,
            BlockBehavior::Lamp
            | BlockBehavior::CopperBulb
            | BlockBehavior::PoweredConsumer
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
                    self.set_state_and_notify(ctx, tick.pos, next, "button_release", None)?;
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
                    let powered = self.diode_input(ctx.world, tick.pos, &state) > 0;
                    if powered != state.bool_property("powered") {
                        let next = self.changed_state(state_id, "powered", powered.to_string())?;
                        self.set_state_and_notify(ctx, tick.pos, next, "repeater_tick", None)?;
                    }
                }
            }
            BlockBehavior::Comparator => {
                let output = self.comparator_output(ctx.world, tick.pos, &state);
                self.comparator_outputs.insert(tick.pos, output);
                let powered = output > 0;
                let mut next = state_id;
                if powered != state.bool_property("powered") {
                    next = self.changed_state(state_id, "powered", powered.to_string())?;
                }
                self.set_state_and_notify(ctx, tick.pos, next, "comparator_tick", None)?;
            }
            BlockBehavior::Observer => {
                let powered = !state.bool_property("powered");
                let next = self.changed_state(state_id, "powered", powered.to_string())?;
                self.set_state_and_notify(ctx, tick.pos, next, "observer_pulse", None)?;
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
                    self.set_state_and_notify(ctx, tick.pos, next, "lectern_pulse_end", None)?;
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
            && let Err(error) = self.move_piston(ctx, event.pos, state_id, event.param_a == 0)
        {
            debug!(?error, pos = ?event.pos, "活塞事件未执行");
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

    fn tick_block_entities(&mut self, ctx: &mut EventContext<'_>) -> Result<(), RulesError> {
        let positions = ctx
            .world
            .block_entities()
            .map(|(pos, _)| *pos)
            .collect::<Vec<_>>();
        for pos in positions {
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
                let moved_block_entity = should_settle.then(|| {
                    data.fields
                        .get("moved_block_entity")
                        .and_then(|value| serde_json::from_value(value.clone()).ok())
                });
                if let Some(moved_block_entity) = moved_block_entity {
                    ctx.world.remove_block_entity(pos);
                    if let Some(data) = moved_block_entity {
                        ctx.world.set_block_entity(pos, data);
                    }
                }
                continue;
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
                BlockBehavior::Dropper | BlockBehavior::Dispenser | BlockBehavior::Crafter => {
                    let powered = self.is_powered(ctx.world, pos);
                    let triggered = state.bool_property("triggered");
                    if powered && !triggered {
                        let next = self.changed_state(state.id, "triggered", "true")?;
                        self.set_state_and_notify(ctx, pos, next, "container_trigger", None)?;
                        ctx.schedule_tick(pos, state.kind, 4, TickPriority::Normal);
                    } else if !powered && triggered {
                        let next = self.changed_state(state.id, "triggered", "false")?;
                        self.set_state_and_notify(ctx, pos, next, "container_untrigger", None)?;
                    }
                }
                _ => {}
            }
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
            Probe::EventCount { kind } => {
                ProbeValue::Integer(self.event_counts.get(kind).copied().unwrap_or(0))
            }
        }
    }
}

impl Java26Rules {
    fn tick_minimal_entities(&mut self, ctx: &mut EventContext<'_>) {
        let mut expired = Vec::<EntityId>::new();
        let entity_ids = ctx.world.entities().map(|(id, _)| *id).collect::<Vec<_>>();
        for id in entity_ids {
            if ctx
                .world
                .entity(id)
                .is_none_or(|entity| entity.kind != "minecraft:item")
            {
                continue;
            }
            let Some(fields) = ctx.world.entity_fields_mut(id) else {
                continue;
            };
            let age = fields.get("age").and_then(serde_json::Value::as_i64).unwrap_or(0) + 1;
            let pickup_delay = fields
                .get("pickup_delay")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0)
                .saturating_sub(1);
            fields.insert("age".to_owned(), serde_json::Value::from(age));
            fields.insert(
                "pickup_delay".to_owned(),
                serde_json::Value::from(pickup_delay),
            );
            if age >= 6_000 {
                expired.push(id);
            }
        }
        for id in expired {
            ctx.world.remove_entity(id);
            *self.event_counts.entry("item_despawn".to_owned()).or_default() += 1;
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
            self.set_state_and_notify(ctx, pos, next, "entity_sensor", None)?;
            if matches!(state.behavior, BlockBehavior::Tripwire) {
                self.refresh_tripwire_hooks(ctx, pos, power > 0)?;
            }
        }
        if power > 0 && !ctx.has_scheduled_tick(pos, state.kind) {
            ctx.schedule_tick(pos, state.kind, delay, TickPriority::Normal);
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
                        self.set_state_and_notify(ctx, cursor, next, "tripwire_hook", None)?;
                        break;
                    }
                    _ => break,
                }
            }
        }
        Ok(())
    }
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

fn attached_direction(state: &StateDefinition) -> Direction {
    match state.property("face") {
        Some("ceiling") => Direction::Down,
        Some("floor") => Direction::Up,
        _ => state.direction_property("facing").unwrap_or(Direction::North),
    }
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

fn wire_neighbors(
    world: &SparseWorld,
    pos: BlockPos,
    registry: &Java26Registry,
) -> Vec<BlockPos> {
    let mut result = Vec::new();
    for direction in Direction::UPDATE_ORDER {
        let candidate = pos.relative(direction);
        if registry
            .state(world.get_block(candidate))
            .is_some_and(|state| matches!(state.behavior, BlockBehavior::Wire))
        {
            result.push(candidate);
        }
    }
    for direction in Direction::HORIZONTAL {
        let side = pos.relative(direction);
        let candidate = if registry
            .state(world.get_block(side))
            .is_some_and(|state| state.redstone_conductor)
        {
            side.relative(Direction::Up)
        } else {
            side.relative(Direction::Down)
        };
        if registry
            .state(world.get_block(candidate))
            .is_some_and(|state| matches!(state.behavior, BlockBehavior::Wire))
        {
            result.push(candidate);
        }
    }
    result
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
