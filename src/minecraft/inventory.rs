use crate::core::{BlockEvent, BlockKind, Direction, Position, ScheduledTick, TickPriority, World};

use super::signal::received_signal;

const HOPPER_TRANSFER_DELAY: u64 = 8;

pub(crate) fn handle_neighbor_changed(world: &mut World, position: Position) -> bool {
    if world.state(position).kind != BlockKind::Hopper {
        return false;
    }
    update_hopper_lock(world, position);
    schedule_hopper_if_needed(world, position);
    true
}

pub(crate) fn handle_scheduled_tick(world: &mut World, tick: ScheduledTick) -> bool {
    if tick.block != BlockKind::Hopper || world.state(tick.position).kind != BlockKind::Hopper {
        return false;
    }
    update_hopper_lock(world, tick.position);
    if !world.state(tick.position).powered() {
        world.ensure_inventory(tick.position, 5);
        let direction = world.state(tick.position).facing();
        let destination = tick.position.offset(direction);
        let _ = world.transfer_inventory_unit(tick.position, destination);
        let source = tick.position.offset(Direction::Up);
        let _ = world.transfer_inventory_unit(source, tick.position);
    }
    schedule_hopper_if_needed(world, tick.position);
    true
}

pub(crate) fn handle_block_event(world: &mut World, event: BlockEvent) -> bool {
    let state = world.state(event.position);
    if event.action != 1 || !matches!(state.kind, BlockKind::Dropper | BlockKind::Dispenser) {
        return false;
    }
    let destination = event.position.offset(state.facing());
    let moved = world.transfer_inventory_unit(event.position, destination);
    if !moved && state.kind == BlockKind::Dispenser {
        consume_dispenser_item(world, event.position);
    }
    true
}

fn schedule_hopper_if_needed(world: &mut World, position: Position) {
    world.ensure_inventory(position, 5);
    if world.state(position).powered() {
        return;
    }
    let has_items = world.inventory(position).is_some_and(|inventory| inventory.items > 0)
        || world
            .inventory(position.offset(Direction::Up))
            .is_some_and(|inventory| inventory.items > 0);
    if has_items
        && !world.has_scheduled_tick(position, BlockKind::Hopper)
    {
        world.schedule_tick(position, BlockKind::Hopper, HOPPER_TRANSFER_DELAY, TickPriority::Normal);
    }
}

fn update_hopper_lock(world: &mut World, position: Position) {
    let state = world.state(position);
    let locked = received_signal(world, position) > 0;
    if locked != state.powered() {
        let _ = world.set_state(position, state.with_powered(locked), "hopper_lock");
    }
}

fn consume_dispenser_item(world: &mut World, position: Position) {
    let Some(mut inventory) = world.inventory(position) else {
        return;
    };
    if inventory.items == 0 {
        return;
    }
    inventory.items -= 1;
    world.set_inventory(position, inventory);
    world.update_neighbors_at(position, world.state(position).kind);
}
