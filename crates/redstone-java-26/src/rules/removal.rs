use redstone_core::{BlockPos, BlockStateId, Direction, EventContext, RulesError};

use super::{
    BlockBehavior, Java26Rules, StateDefinition, update_attached_power_neighbors,
    update_torch_output_neighbors,
};

impl Java26Rules {
    pub(super) fn remove_block_after_support_loss(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        cause: &str,
    ) -> Result<BlockStateId, RulesError> {
        let old_state_id = ctx.world.get_block(pos);
        if old_state_id == self.registry.air_state() {
            return Ok(old_state_id);
        }
        let old_state = self.state(old_state_id)?.clone();
        let air = self.registry.air_state();
        self.set_block(ctx, pos, air, cause)?;
        let state = self.apply_observer_lifecycle(ctx, pos, old_state_id, air, true, true)?;
        self.affect_neighbors_after_removal(ctx, pos, &old_state, false)?;
        self.update_indirect_neighbor_shapes(ctx, pos, &old_state)?;
        ctx.update_neighbors(pos, old_state.kind, None, None);
        self.queue_neighbor_shape_updates(ctx, pos);
        Ok(state)
    }

    pub(super) fn affect_neighbors_after_removal(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
        moved_by_piston: bool,
    ) -> Result<(), RulesError> {
        if moved_by_piston {
            return Ok(());
        }
        match state.behavior {
            BlockBehavior::Wire => {
                for direction in [
                    Direction::Down,
                    Direction::Up,
                    Direction::North,
                    Direction::South,
                    Direction::West,
                    Direction::East,
                ] {
                    ctx.update_neighbors(pos.relative(direction), state.kind, None, None);
                }
            }
            BlockBehavior::Torch { .. } => {
                update_torch_output_neighbors(ctx, pos, state);
            }
            BlockBehavior::Repeater | BlockBehavior::Comparator => {
                self.update_diode_output_neighbors(ctx, pos, state);
            }
            BlockBehavior::Lever | BlockBehavior::Button { .. }
                if state.bool_property("powered") =>
            {
                update_attached_power_neighbors(ctx, pos, state);
            }
            BlockBehavior::PressurePlate { .. }
                if state.bool_property("powered")
                    || state.int_property("power").unwrap_or(0) > 0 =>
            {
                ctx.update_neighbors(pos, state.kind, None, None);
                ctx.update_neighbors(pos.relative(Direction::Down), state.kind, None, None);
            }
            BlockBehavior::TripwireHook
                if state.bool_property("attached") || state.bool_property("powered") =>
            {
                let front = state.facing.unwrap_or(Direction::North).opposite();
                ctx.update_neighbors(pos, state.kind, None, None);
                ctx.update_neighbors(pos.relative(front), state.kind, None, None);
            }
            _ => {}
        }
        Ok(())
    }
}
