use redstone_core::{BlockPos, Direction};
use redstone_java_26::StateDefinition;

use super::super::mesh::{Color, Transform, Vertex, cuboid, ribbon, rod_transformed};
use super::{active_color, facing};

pub(super) fn lever(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let normal = mount_normal(state);
    let transform = Transform::mounted_facing(normal, facing(state));
    cuboid(
        output,
        pos,
        [0.31, 0.0, 0.22],
        [0.69, 0.12, 0.78],
        color,
        None,
        transform,
    );
    let end_z = if state.powered { 0.29 } else { 0.71 };
    rod_transformed(
        output,
        pos,
        [0.5, 0.1, 0.5],
        [0.5, 0.66, end_z],
        0.115,
        active_color([0.62, 0.49, 0.3, 0.0], state.powered),
        transform,
    );
}

pub(super) fn button(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    wooden: bool,
    color: Color,
) {
    let normal = mount_normal(state);
    let transform = Transform::mounted_facing(normal, facing(state));
    let height = if state.powered { 0.055 } else { 0.095 };
    let width = if wooden { 0.48 } else { 0.38 };
    cuboid(
        output,
        pos,
        [0.5 - width * 0.5, 0.0, 0.34],
        [0.5 + width * 0.5, height, 0.66],
        active_color(color, state.powered),
        None,
        transform,
    );
}

pub(super) fn pressure_plate(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let pressed = state.power > 0 || state.powered;
    cuboid(
        output,
        pos,
        [0.0625, 0.0, 0.0625],
        [0.9375, if pressed { 0.035 } else { 0.0625 }, 0.9375],
        active_color(color, pressed),
        None,
        Transform::identity(),
    );
}

pub(super) fn tripwire(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let color = active_color(color, state.powered);
    let height = if state.bool_property("attached") { 0.055 } else { 0.085 };
    let mut any = false;
    for (name, end) in [
        ("north", [0.5, height, 0.0]),
        ("south", [0.5, height, 1.0]),
        ("west", [0.0, height, 0.5]),
        ("east", [1.0, height, 0.5]),
    ] {
        if state.bool_property(name) {
            ribbon(output, pos, [0.5, height, 0.5], end, 0.025, color);
            any = true;
        }
    }
    if !any {
        ribbon(
            output,
            pos,
            [0.18, height, 0.5],
            [0.82, height, 0.5],
            0.025,
            color,
        );
    }
}

pub(super) fn tripwire_hook(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = Transform::mounted_facing(facing(state), Direction::Up);
    cuboid(
        output,
        pos,
        [0.3, 0.0, 0.28],
        [0.7, 0.12, 0.72],
        color,
        None,
        transform,
    );
    let end_z = if state.bool_property("attached") { 0.72 } else { 0.6 };
    rod_transformed(
        output,
        pos,
        [0.5, 0.1, 0.5],
        [0.5, 0.35, end_z],
        0.08,
        active_color([0.55, 0.57, 0.58, 0.0], state.powered),
        transform,
    );
}

pub(super) fn door(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let mut direction = facing(state);
    if state.bool_property("open") {
        direction = if state.property("hinge") == Some("right") {
            direction.clockwise()
        } else {
            direction.counter_clockwise()
        };
    }
    let transform = Transform::facing(direction);
    cuboid(
        output,
        pos,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 0.1875],
        active_color(color, state.powered),
        None,
        transform,
    );
    if state.property("half") == Some("lower") {
        cuboid(
            output,
            pos,
            [0.78, 0.42, 0.17],
            [0.86, 0.5, 0.25],
            [0.72, 0.62, 0.24, 0.0],
            None,
            transform,
        );
    }
}

pub(super) fn powered_consumer(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    if path.ends_with("_trapdoor") {
        trapdoor(output, pos, state, color);
    } else {
        fence_gate(output, pos, state, color);
    }
}

fn trapdoor(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = if state.bool_property("open") {
        Transform::facing(facing(state))
    } else {
        let y = if state.property("half") == Some("top") {
            0.8125
        } else {
            0.0
        };
        cuboid(
            output,
            pos,
            [0.0, y, 0.0],
            [1.0, y + 0.1875, 1.0],
            active_color(color, state.powered),
            None,
            Transform::identity(),
        );
        return;
    };
    cuboid(
        output,
        pos,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 0.1875],
        active_color(color, state.powered),
        None,
        transform,
    );
}

fn fence_gate(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = Transform::facing(facing(state));
    for x in [0.0, 0.8125] {
        cuboid(
            output,
            pos,
            [x, 0.0, 0.375],
            [x + 0.1875, 1.0, 0.625],
            color,
            None,
            transform,
        );
    }
    if !state.bool_property("open") {
        for y in [0.28, 0.66] {
            cuboid(
                output,
                pos,
                [0.15, y, 0.43],
                [0.85, y + 0.12, 0.57],
                active_color(color, state.powered),
                None,
                transform,
            );
        }
    }
}

fn mount_normal(state: &StateDefinition) -> Direction {
    match state.property("face") {
        Some("ceiling") => Direction::Down,
        Some("wall") => facing(state),
        _ => Direction::Up,
    }
}
