mod piston;
mod signal;

pub(crate) use signal::{set_external_power, set_external_signal, toggle_lever, trigger_button};

use crate::core::{BlockEvent, BlockKind, Position, ScheduledTick, World};

pub trait BlockBehavior {
    fn on_neighbor_changed(
        &self,
        world: &mut World,
        position: Position,
        changed_block: BlockKind,
        source: Position,
    );

    fn on_scheduled_tick(&self, world: &mut World, tick: ScheduledTick);

    fn on_block_event(&self, world: &mut World, event: BlockEvent);
}

pub struct VanillaBehavior;

impl BlockBehavior for VanillaBehavior {
    fn on_neighbor_changed(
        &self,
        world: &mut World,
        position: Position,
        changed_block: BlockKind,
        source: Position,
    ) {
        signal::handle_neighbor_changed(world, position, changed_block, source);
    }

    fn on_scheduled_tick(&self, world: &mut World, tick: ScheduledTick) {
        signal::handle_scheduled_tick(world, tick);
    }

    fn on_block_event(&self, world: &mut World, event: BlockEvent) {
        piston::handle_block_event(world, event);
    }
}

pub(crate) fn handle_neighbor_changed(
    world: &mut World,
    position: Position,
    changed_block: BlockKind,
    source: Position,
) {
    VanillaBehavior.on_neighbor_changed(world, position, changed_block, source);
}

pub(crate) fn handle_scheduled_tick(world: &mut World, tick: ScheduledTick) {
    VanillaBehavior.on_scheduled_tick(world, tick);
}

pub(crate) fn handle_block_event(world: &mut World, event: BlockEvent) {
    VanillaBehavior.on_block_event(world, event);
}
