use redstone_core::BlockPos;
use redstone_java_26::StateDefinition;

use super::super::mesh::{Color, Transform, Vertex, cuboid, quad, rod};
use super::{active_color, facing};

pub(super) fn wire(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let height = 0.028;
    wire_top(output, pos, [0.375, 0.375], [0.625, 0.625], height, color);
    for (name, min, max, wall_min, wall_max) in [
        ("north", [0.455, 0.002, 0.0], [0.545, height, 0.5], [0.455, 0.0, 0.0], [0.545, 1.0, 0.025]),
        ("south", [0.455, 0.002, 0.5], [0.545, height, 1.0], [0.455, 0.0, 0.975], [0.545, 1.0, 1.0]),
        ("west", [0.0, 0.002, 0.455], [0.5, height, 0.545], [0.0, 0.0, 0.455], [0.025, 1.0, 0.545]),
        ("east", [0.5, 0.002, 0.455], [1.0, height, 0.545], [0.975, 0.0, 0.455], [1.0, 1.0, 0.545]),
    ] {
        match state.property(name) {
            Some("side") => wire_top(
                output,
                pos,
                [min[0], min[2]],
                [max[0], max[2]],
                height,
                color,
            ),
            Some("up") => {
                wire_top(
                    output,
                    pos,
                    [min[0], min[2]],
                    [max[0], max[2]],
                    height,
                    color,
                );
                wire_wall(output, pos, name, wall_min, wall_max, color);
            }
            _ => {}
        }
    }
}

fn wire_top(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    min: [f32; 2],
    max: [f32; 2],
    height: f32,
    color: Color,
) {
    quad(
        output,
        pos,
        [
            [min[0], height, min[1]],
            [min[0], height, max[1]],
            [max[0], height, max[1]],
            [max[0], height, min[1]],
        ],
        [0.0, 1.0, 0.0],
        color,
        Transform::identity(),
        false,
    );
}

fn wire_wall(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    direction: &str,
    min: [f32; 3],
    max: [f32; 3],
    color: Color,
) {
    let (normal, corners) = match direction {
        "north" => ([0.0, 0.0, -1.0], [[max[0], min[1], min[2]], [min[0], min[1], min[2]], [min[0], max[1], min[2]], [max[0], max[1], min[2]]]),
        "south" => ([0.0, 0.0, 1.0], [[min[0], min[1], max[2]], [max[0], min[1], max[2]], [max[0], max[1], max[2]], [min[0], max[1], max[2]]]),
        "west" => ([-1.0, 0.0, 0.0], [[min[0], min[1], min[2]], [min[0], min[1], max[2]], [min[0], max[1], max[2]], [min[0], max[1], min[2]]]),
        "east" => ([1.0, 0.0, 0.0], [[max[0], min[1], max[2]], [max[0], min[1], min[2]], [max[0], max[1], min[2]], [max[0], max[1], max[2]]]),
        _ => return,
    };
    quad(
        output,
        pos,
        corners,
        normal,
        color,
        Transform::identity(),
        true,
    );
}

pub(super) fn torch(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    wall: bool,
    color: Color,
) {
    let stem = [0.38, 0.22, 0.09, 0.0];
    let flame = active_color(color, state.lit);
    if wall {
        let (x, y, z) = facing(state).step();
        let vector = [x as f32, y as f32, z as f32];
        let start = [
            0.5 - vector[0] * 0.42,
            0.38,
            0.5 - vector[2] * 0.42,
        ];
        let end = [
            0.5 + vector[0] * 0.12,
            0.78,
            0.5 + vector[2] * 0.12,
        ];
        rod(output, pos, start, end, 0.11, stem);
        cuboid(
            output,
            pos,
            [end[0] - 0.095, end[1] - 0.06, end[2] - 0.095],
            [end[0] + 0.095, end[1] + 0.12, end[2] + 0.095],
            flame,
            None,
            Transform::identity(),
        );
    } else {
        cuboid(
            output,
            pos,
            [0.445, 0.0, 0.445],
            [0.555, 0.68, 0.555],
            stem,
            None,
            Transform::identity(),
        );
        cuboid(
            output,
            pos,
            [0.405, 0.62, 0.405],
            [0.595, 0.82, 0.595],
            flame,
            None,
            Transform::identity(),
        );
    }
}

pub(super) fn repeater(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = Transform::facing(facing(state));
    plate(output, pos, color, transform);
    let active = active_color([0.88, 0.08, 0.04, 0.0], state.powered);
    small_torch(output, pos, [0.5, 0.34], active, transform);
    let delay = state.int_property("delay").unwrap_or(1).clamp(1, 4) as f32;
    small_torch(
        output,
        pos,
        [0.5, 0.53 + delay * 0.075],
        [0.58, 0.08, 0.04, 0.0],
        transform,
    );
    if state.bool_property("locked") {
        cuboid(
            output,
            pos,
            [0.19, 0.14, 0.48],
            [0.81, 0.23, 0.57],
            [0.42, 0.45, 0.48, 0.0],
            None,
            transform,
        );
    }
}

pub(super) fn comparator(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = Transform::facing(facing(state));
    plate(output, pos, color, transform);
    let active = active_color([0.92, 0.08, 0.04, 0.0], state.powered);
    let subtract = state.property("mode") == Some("subtract");
    small_torch(output, pos, [0.5, 0.31], active, transform);
    small_torch(output, pos, [0.31, 0.69], [0.58, 0.08, 0.04, 0.0], transform);
    small_torch(output, pos, [0.69, 0.69], [0.58, 0.08, 0.04, 0.0], transform);
    if subtract {
        cuboid(
            output,
            pos,
            [0.455, 0.25, 0.265],
            [0.545, 0.34, 0.355],
            active,
            None,
            transform,
        );
    }
}

fn plate(output: &mut Vec<Vertex>, pos: BlockPos, color: Color, transform: Transform) {
    cuboid(
        output,
        pos,
        [0.0, 0.0, 0.0],
        [1.0, 0.125, 1.0],
        color,
        None,
        transform,
    );
    cuboid(
        output,
        pos,
        [0.08, 0.125, 0.08],
        [0.92, 0.145, 0.92],
        [0.68, 0.05, 0.03, 0.0],
        None,
        transform,
    );
}

fn small_torch(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    xz: [f32; 2],
    color: Color,
    transform: Transform,
) {
    cuboid(
        output,
        pos,
        [xz[0] - 0.045, 0.14, xz[1] - 0.045],
        [xz[0] + 0.045, 0.39, xz[1] + 0.045],
        color,
        None,
        transform,
    );
}
