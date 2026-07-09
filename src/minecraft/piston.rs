use std::collections::{BTreeSet, VecDeque};

use crate::core::{BlockEvent, BlockKind, BlockState, Direction, Position, World};

use super::signal::{queue_piston_event, signal_from};

pub(crate) fn check_piston(world: &mut World, position: Position, state: BlockState) {
    let powered = piston_powered(world, position, state.facing());
    if powered && !state.extended() {
        queue_piston_event(world, position, 0, state.facing());
    } else if !powered && state.extended() {
        queue_piston_event(world, position, 1, state.facing());
    }
}

pub(crate) fn handle_block_event(world: &mut World, event: BlockEvent) {
    let state = world.state(event.position);
    if !state.kind.is_piston() {
        return;
    }
    let facing = state.facing();
    match event.action {
        0 if piston_powered(world, event.position, facing) && !state.extended() => {
            extend(world, event.position, state);
        }
        1 if !piston_powered(world, event.position, facing) && state.extended() => {
            retract(world, event.position, state);
        }
        _ => {}
    }
}

fn piston_powered(world: &World, position: Position, push_direction: Direction) -> bool {
    for direction in Direction::ALL {
        if direction != push_direction {
            let source = position.offset(direction);
            if signal_from(world, source, direction.opposite()) > 0 {
                return true;
            }
        }
    }
    if signal_from(world, position, Direction::Down) > 0 {
        return true;
    }
    let above = position.offset(Direction::Up);
    Direction::ALL.into_iter().any(|direction| {
        direction != Direction::Down
            && signal_from(world, above.offset(direction), direction.opposite()) > 0
    })
}

fn extend(world: &mut World, piston: Position, state: BlockState) {
    let direction = state.facing();
    let Some(to_move) = resolve_push(world, piston, direction) else {
        return;
    };
    for position in &to_move {
        world.move_state_silent(*position, position.offset(direction), "piston_extend".to_owned());
    }
    let head = piston.offset(direction);
    world.set_state_silent(
        head,
        BlockState::new(BlockKind::PistonHead).with_facing(direction),
        "piston_head_extend".to_owned(),
    );
    world.set_state_silent(piston, state.with_extended(true), "piston_extend".to_owned());
    world.update_neighbors_at(piston, state.kind);
    world.update_neighbors_at(head, BlockKind::PistonHead);
    for position in to_move {
        world.update_neighbors_at(position, BlockKind::Air);
        world.update_neighbors_at(position.offset(direction), world.state(position.offset(direction)).kind);
    }
}

fn retract(world: &mut World, piston: Position, state: BlockState) {
    let direction = state.facing();
    let arm = piston.offset(direction);
    world.set_state_silent(arm, BlockState::new(BlockKind::Air), "piston_head_retract".to_owned());
    if state.kind == BlockKind::StickyPiston {
        let pulled = piston.relative(direction, 2);
        let pulled_state = world.state(pulled);
        if movable(pulled_state) {
            world.move_state_silent(pulled, arm, "sticky_piston_retract".to_owned());
            world.update_neighbors_at(pulled, BlockKind::Air);
            world.update_neighbors_at(arm, pulled_state.kind);
        }
    }
    world.set_state_silent(piston, state.with_extended(false), "piston_retract".to_owned());
    world.update_neighbors_at(piston, state.kind);
    world.update_neighbors_at(arm, BlockKind::Air);
}

fn resolve_push(world: &World, piston: Position, direction: Direction) -> Option<Vec<Position>> {
    let first = piston.offset(direction);
    if world.state(first).kind == BlockKind::Air {
        return Some(Vec::new());
    }
    let mut positions = BTreeSet::new();
    let mut pending = VecDeque::from([first]);
    while let Some(position) = pending.pop_front() {
        if !positions.insert(position) {
            continue;
        }
        if positions.len() > 12 {
            return None;
        }
        let state = world.state(position);
        if !movable(state) {
            return None;
        }
        if state.kind.is_sticky() {
            for neighbor_direction in Direction::ALL {
                let neighbor = position.offset(neighbor_direction);
                let neighbor_state = world.state(neighbor);
                if sticks_to(state.kind, neighbor_state.kind) && neighbor_state.kind != BlockKind::Air {
                    pending.push_back(neighbor);
                }
            }
        }
        let ahead = position.offset(direction);
        let ahead_state = world.state(ahead);
        if ahead_state.kind != BlockKind::Air && !positions.contains(&ahead) {
            pending.push_back(ahead);
        }
    }
    let mut ordered = positions.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|position| -projection(*position, piston, direction));
    Some(ordered)
}

fn movable(state: BlockState) -> bool {
    state.kind != BlockKind::Air && !state.kind.is_immovable() && !(state.kind.is_piston() && state.extended())
}

fn sticks_to(left: BlockKind, right: BlockKind) -> bool {
    left.is_sticky() && right.is_sticky() && left == right
}

fn projection(position: Position, origin: Position, direction: Direction) -> i32 {
    let dx = position.x - origin.x;
    let dy = position.y - origin.y;
    let dz = position.z - origin.z;
    dx * direction.step_x() + dy * direction.step_y() + dz * direction.step_z()
}
