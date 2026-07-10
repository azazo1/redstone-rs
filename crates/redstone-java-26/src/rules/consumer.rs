use redstone_core::{
    BlockPos, BlockStateId, Direction, EventContext, RulesError, SparseWorld, TickPriority,
};

use super::{BlockBehavior, Java26Rules, StateDefinition};

impl Java26Rules {
    pub(super) fn refresh_door(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let other_pos = if state.property("half") == Some("lower") {
            pos.relative(Direction::Up)
        } else {
            pos.relative(Direction::Down)
        };
        let powered = self.is_powered(ctx.world, pos) || self.is_powered(ctx.world, other_pos);
        let mut next = state_id;
        if state.bool_property("powered") != powered {
            next = self.changed_state(next, "powered", powered.to_string())?;
        }
        if state.bool_property("open") != powered {
            next = self.changed_state(next, "open", powered.to_string())?;
            *self
                .event_counts
                .entry(if powered { "door_open" } else { "door_close" }.to_owned())
                .or_default() += 1;
        }
        self.set_state_and_notify(ctx, pos, next, "door_power", None)?;

        let other_id = ctx.world.get_block(other_pos);
        let other = self.state(other_id)?.clone();
        if other.name == state.name && other.property("half") != state.property("half") {
            let mut other_next = other_id;
            if other.bool_property("powered") != powered {
                other_next = self.changed_state(other_next, "powered", powered.to_string())?;
            }
            if other.bool_property("open") != powered {
                other_next = self.changed_state(other_next, "open", powered.to_string())?;
            }
            self.set_state_and_notify(ctx, other_pos, other_next, "door_pair_power", None)?;
        }
        Ok(())
    }

    pub(super) fn refresh_powered_rail(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = self.is_powered(ctx.world, pos)
            || self.find_powered_rail_signal(ctx.world, pos, &state, true, 0)
            || self.find_powered_rail_signal(ctx.world, pos, &state, false, 0);
        if powered != state.bool_property("powered") {
            let next = self.changed_state(state_id, "powered", powered.to_string())?;
            self.set_state_and_notify(ctx, pos, next, "powered_rail", None)?;
        }
        Ok(())
    }

    fn find_powered_rail_signal(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        state: &StateDefinition,
        forward: bool,
        depth: usize,
    ) -> bool {
        if depth >= 8 {
            return false;
        }
        let Some((next_pos, axis, check_below)) = next_rail_pos(pos, state, forward) else {
            return false;
        };
        self.same_rail_has_power(world, next_pos, state, forward, depth, axis)
            || check_below
                && self.same_rail_has_power(
                    world,
                    next_pos.relative(Direction::Down),
                    state,
                    forward,
                    depth,
                    axis,
                )
    }

    fn same_rail_has_power(
        &self,
        world: &SparseWorld,
        pos: BlockPos,
        origin: &StateDefinition,
        forward: bool,
        depth: usize,
        axis: RailAxis,
    ) -> bool {
        let Ok(state) = self.state(world.get_block(pos)) else {
            return false;
        };
        if state.name != origin.name
            || !matches!(state.behavior, BlockBehavior::PoweredRail)
            || rail_axis(state.property("shape")) != Some(axis)
            || !state.bool_property("powered")
        {
            return false;
        }
        self.is_powered(world, pos)
            || self.find_powered_rail_signal(world, pos, state, forward, depth + 1)
    }

    pub(super) fn refresh_edge_consumer(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        let powered = self.is_powered(ctx.world, pos);
        if powered == state.bool_property("powered") {
            return Ok(());
        }
        let next = self.changed_state(state_id, "powered", powered.to_string())?;
        self.set_state_and_notify(ctx, pos, next, "edge_consumer", None)?;
        if powered {
            let event = if matches!(state.behavior, BlockBehavior::NoteBlock) {
                "note_block_play"
            } else {
                "bell_ring"
            };
            *self.event_counts.entry(event.to_owned()).or_default() += 1;
        }
        Ok(())
    }

    pub(super) fn hit_target(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        face: Direction,
        location: [f64; 3],
        arrow: bool,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(pos);
        let state = self.state(state_id)?.clone();
        if !matches!(state.behavior, BlockBehavior::Target) {
            return Err(RulesError::Message(format!(
                "目标命中动作与方块不匹配: {} at {pos:?}",
                state.name
            )));
        }
        if ctx.has_scheduled_tick(pos, state.kind) {
            return Ok(());
        }
        let fractions = location.map(|value| (value - value.floor() - 0.5).abs());
        let distance = match face {
            Direction::Down | Direction::Up => fractions[0].max(fractions[2]),
            Direction::North | Direction::South => fractions[0].max(fractions[1]),
            Direction::West | Direction::East => fractions[1].max(fractions[2]),
        };
        let strength = (15.0 * ((0.5 - distance) / 0.5).clamp(0.0, 1.0)).ceil() as i32;
        let strength = strength.clamp(1, 15);
        let next = self.changed_state(state_id, "power", strength.to_string())?;
        self.set_state_and_notify(ctx, pos, next, "target_hit", None)?;
        ctx.schedule_tick(
            pos,
            state.kind,
            if arrow { 20 } else { 8 },
            TickPriority::Normal,
        );
        *self.event_counts.entry("target_hit".to_owned()).or_default() += 1;
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum RailAxis {
    EastWest,
    NorthSouth,
}

fn rail_axis(shape: Option<&str>) -> Option<RailAxis> {
    match shape? {
        "east_west" | "ascending_east" | "ascending_west" => Some(RailAxis::EastWest),
        "north_south" | "ascending_north" | "ascending_south" => Some(RailAxis::NorthSouth),
        _ => None,
    }
}

fn next_rail_pos(
    pos: BlockPos,
    state: &StateDefinition,
    forward: bool,
) -> Option<(BlockPos, RailAxis, bool)> {
    let shape = state.property("shape")?;
    let (offset, axis, check_below) = match (shape, forward) {
        ("north_south", true) => ((0, 0, 1), RailAxis::NorthSouth, true),
        ("north_south", false) => ((0, 0, -1), RailAxis::NorthSouth, true),
        ("east_west", true) => ((-1, 0, 0), RailAxis::EastWest, true),
        ("east_west", false) => ((1, 0, 0), RailAxis::EastWest, true),
        ("ascending_east", true) => ((-1, 0, 0), RailAxis::EastWest, true),
        ("ascending_east", false) => ((1, 1, 0), RailAxis::EastWest, false),
        ("ascending_west", true) => ((-1, 1, 0), RailAxis::EastWest, false),
        ("ascending_west", false) => ((1, 0, 0), RailAxis::EastWest, true),
        ("ascending_north", true) => ((0, 0, 1), RailAxis::NorthSouth, true),
        ("ascending_north", false) => ((0, 1, -1), RailAxis::NorthSouth, false),
        ("ascending_south", true) => ((0, 1, 1), RailAxis::NorthSouth, false),
        ("ascending_south", false) => ((0, 0, -1), RailAxis::NorthSouth, true),
        _ => return None,
    };
    Some((pos.offset(offset.0, offset.1, offset.2), axis, check_below))
}
