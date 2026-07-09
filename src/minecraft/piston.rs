use std::collections::{BTreeSet, VecDeque};

use crate::core::{BlockEvent, BlockKind, BlockState, Direction, PistonMotion, Position, World};

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
    let Some(plan) = resolve_push(world, piston, direction) else {
        return;
    };
    for position in &plan.to_destroy {
        world.set_state_silent(*position, BlockState::new(BlockKind::Air), "piston_destroy".to_owned());
        world.update_neighbors_at(*position, BlockKind::Air);
    }
    let to_move = plan.to_move;
    let moved = to_move
        .iter()
        .map(|position| (*position, world.state(*position), world.moving_block_data(*position)))
        .collect::<Vec<_>>();
    for (position, moved_state, moved_data) in moved {
        let destination = position.offset(direction);
        world.set_state_silent(
            destination,
            BlockState::new(BlockKind::MovingPiston).with_facing(direction),
            "piston_start_extend".to_owned(),
        );
        world.start_piston_motion(
            destination,
            PistonMotion {
                moved_state,
                moved_data,
                direction,
                extending: true,
                source: false,
                progress: 0,
            },
        );
        world.set_state_silent(position, BlockState::new(BlockKind::Air), "piston_start_extend".to_owned());
    }
    let head = piston.offset(direction);
    world.set_state_silent(
        head,
        BlockState::new(BlockKind::MovingPiston).with_facing(direction),
        "piston_head_start_extend".to_owned(),
    );
    world.start_piston_motion(
        head,
        PistonMotion {
            moved_state: BlockState::new(BlockKind::PistonHead).with_facing(direction),
            moved_data: None,
            direction,
            extending: true,
            source: true,
            progress: 0,
        },
    );
    world.set_state_silent(piston, state.with_extended(true), "piston_extend".to_owned());
    world.update_neighbors_at(piston, state.kind);
    world.update_neighbors_at(head, BlockKind::MovingPiston);
    for position in to_move {
        world.update_neighbors_at(position, BlockKind::Air);
        world.update_neighbors_at(position.offset(direction), BlockKind::MovingPiston);
    }
}

fn retract(world: &mut World, piston: Position, state: BlockState) {
    let direction = state.facing();
    let arm = piston.offset(direction);
    if world.state(arm).kind == BlockKind::MovingPiston {
        world.complete_piston_motion(arm);
    }
    world.set_state_silent(arm, BlockState::new(BlockKind::Air), "piston_head_retract".to_owned());
    if state.kind == BlockKind::StickyPiston {
        let pulled = piston.relative(direction, 2);
        let pulled_state = world.state(pulled);
        let piston_piece = if pulled_state.kind == BlockKind::MovingPiston {
            world
                .complete_piston_motion(pulled)
                .is_some_and(|motion| motion.extending && motion.direction == direction)
        } else {
            false
        };
        if !piston_piece && movable(world.state(pulled)) {
            let moved_state = world.state(pulled);
            let moved_data = world.moving_block_data(pulled);
            world.set_state_silent(
                arm,
                BlockState::new(BlockKind::MovingPiston).with_facing(direction),
                "sticky_piston_start_retract".to_owned(),
            );
            world.start_piston_motion(
                arm,
                PistonMotion {
                    moved_state,
                    moved_data,
                    direction,
                    extending: false,
                    source: false,
                    progress: 0,
                },
            );
            world.set_state_silent(pulled, BlockState::new(BlockKind::Air), "sticky_piston_start_retract".to_owned());
            world.update_neighbors_at(pulled, BlockKind::Air);
            world.update_neighbors_at(arm, BlockKind::MovingPiston);
        }
    }
    world.set_state_silent(piston, state.with_extended(false), "piston_retract".to_owned());
    world.update_neighbors_at(piston, state.kind);
    world.update_neighbors_at(arm, world.state(arm).kind);
}

struct PushPlan {
    to_move: Vec<Position>,
    to_destroy: Vec<Position>,
}

fn resolve_push(world: &World, piston: Position, direction: Direction) -> Option<PushPlan> {
    let first = piston.offset(direction);
    if world.state(first).kind == BlockKind::Air {
        return Some(PushPlan {
            to_move: Vec::new(),
            to_destroy: Vec::new(),
        });
    }
    let mut positions = BTreeSet::new();
    let mut to_destroy = BTreeSet::new();
    let mut pending = VecDeque::from([first]);
    while let Some(position) = pending.pop_front() {
        if positions.contains(&position) || to_destroy.contains(&position) {
            continue;
        }
        let state = world.state(position);
        if state.kind.is_piston_destroyable() {
            to_destroy.insert(position);
            continue;
        }
        if !positions.insert(position) {
            continue;
        }
        if positions.len() > 12 {
            return None;
        }
        if !movable(state) {
            return None;
        }
        if state.kind.is_sticky() {
            for neighbor_direction in Direction::ALL {
                if same_axis(neighbor_direction, direction) {
                    continue;
                }
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
    Some(PushPlan {
        to_move: ordered,
        to_destroy: to_destroy.into_iter().collect(),
    })
}

fn movable(state: BlockState) -> bool {
    state.kind != BlockKind::Air && !state.kind.is_immovable() && !(state.kind.is_piston() && state.extended())
}

fn sticks_to(left: BlockKind, right: BlockKind) -> bool {
    !(matches!((left, right), (BlockKind::SlimeBlock, BlockKind::HoneyBlock) | (BlockKind::HoneyBlock, BlockKind::SlimeBlock)))
}

fn same_axis(left: Direction, right: Direction) -> bool {
    (left.step_x() != 0 && right.step_x() != 0)
        || (left.step_y() != 0 && right.step_y() != 0)
        || (left.step_z() != 0 && right.step_z() != 0)
}

fn projection(position: Position, origin: Position, direction: Direction) -> i32 {
    let dx = position.x - origin.x;
    let dy = position.y - origin.y;
    let dz = position.z - origin.z;
    dx * direction.step_x() + dy * direction.step_y() + dz * direction.step_z()
}
