use redstone_core::{BlockPos, BlockStateId, Direction, EventContext, RulesError, TickPriority};

use super::{BlockBehavior, Java26Rules};

impl Java26Rules {
    pub(super) fn update_observer_shape(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        source_pos: BlockPos,
    ) -> Result<(), RulesError> {
        let state_id = ctx.world.get_block(pos);
        self.refresh_observer(ctx, pos, state_id, source_pos)
    }

    pub(super) fn update_moved_observer_from_neighbor_shapes(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?.clone();
        if matches!(state.behavior, BlockBehavior::Observer) {
            self.start_observer_signal(ctx, pos, &state)?;
        }
        Ok(())
    }

    pub(super) fn apply_observer_lifecycle(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        old_state: BlockStateId,
        new_state: BlockStateId,
        affect_removal: bool,
        run_on_place: bool,
    ) -> Result<BlockStateId, RulesError> {
        let old = self.state(old_state)?.clone();
        let new = self.state(new_state)?.clone();
        if old.kind == new.kind {
            return Ok(new_state);
        }
        if affect_removal
            && matches!(old.behavior, BlockBehavior::Observer)
            && old.bool_property("powered")
            && ctx.has_scheduled_tick(pos, old.kind)
        {
            self.update_diode_output_neighbors(ctx, pos, &old);
        }
        if run_on_place
            && matches!(new.behavior, BlockBehavior::Observer)
            && new.bool_property("powered")
            && !ctx.has_scheduled_tick(pos, new.kind)
        {
            let reset = self.changed_state(new_state, "powered", "false")?;
            self.set_block(ctx, pos, reset, "observer_place_reset")?;
            let reset_state = self.state(reset)?.clone();
            self.update_diode_output_neighbors(ctx, pos, &reset_state);
            return Ok(reset);
        }
        Ok(new_state)
    }

    fn refresh_observer(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state_id: BlockStateId,
        source_pos: BlockPos,
    ) -> Result<(), RulesError> {
        let state = self.state(state_id)?;
        if !matches!(state.behavior, BlockBehavior::Observer) {
            return Ok(());
        }
        let facing = state.facing.unwrap_or(Direction::South);
        if pos.relative(facing) == source_pos {
            let state = state.clone();
            self.start_observer_signal(ctx, pos, &state)?;
        }
        Ok(())
    }

    fn start_observer_signal(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &super::StateDefinition,
    ) -> Result<(), RulesError> {
        if !state.bool_property("powered") && !ctx.has_scheduled_tick(pos, state.kind) {
            ctx.schedule_tick(pos, state.kind, 2, TickPriority::Normal);
        }
        Ok(())
    }
}
