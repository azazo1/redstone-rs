use redstone_core::BlockPos;
use redstone_java_26::StateDefinition;

use super::super::mesh::{Color, Transform, Vertex, cuboid, rod};
use super::{active_color, facing};

pub(super) fn observer(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let transform = Transform::facing(facing(state));
    cuboid(
        output,
        pos,
        [0.0; 3],
        [1.0; 3],
        color,
        None,
        transform,
    );
    cuboid(
        output,
        pos,
        [0.22, 0.22, -0.012],
        [0.78, 0.78, 0.035],
        [0.28, 0.3, 0.31, 0.0],
        None,
        transform,
    );
    cuboid(
        output,
        pos,
        [0.36, 0.36, 0.965],
        [0.64, 0.64, 1.012],
        active_color([0.78, 0.08, 0.04, 0.0], state.powered),
        None,
        transform,
    );
}

pub(super) fn piston(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    sticky: bool,
    color: Color,
) {
    piston_with_extended(
        output,
        pos,
        state,
        sticky,
        color,
        state.bool_property("extended"),
    );
}

pub(super) fn extended_piston(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    sticky: bool,
    color: Color,
) {
    piston_with_extended(output, pos, state, sticky, color, true);
}

fn piston_with_extended(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    sticky: bool,
    color: Color,
    extended: bool,
) {
    let transform = Transform::facing(facing(state));
    cuboid(
        output,
        pos,
        [0.0; 3],
        [1.0; 3],
        color,
        None,
        transform,
    );
    let face_color = if sticky {
        [0.34, 0.62, 0.25, 0.0]
    } else {
        [0.76, 0.65, 0.4, 0.0]
    };
    cuboid(
        output,
        pos,
        [0.125, 0.125, -0.012],
        [0.875, 0.875, 0.055],
        face_color,
        None,
        transform,
    );
    if extended {
        cuboid(
            output,
            pos,
            [0.31, 0.31, -0.02],
            [0.69, 0.69, 0.18],
            [0.22, 0.18, 0.12, 0.0],
            None,
            transform,
        );
    }
}

pub(super) fn piston_head(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    transform: Transform,
) {
    let sticky = state.property("type") == Some("sticky")
        || matches!(state.behavior, redstone_java_26::BlockBehavior::Piston { sticky: true });
    let head_color = if sticky {
        [0.34, 0.62, 0.25, 0.0]
    } else {
        [0.76, 0.65, 0.4, 0.0]
    };
    let short = state.bool_property("short");
    cuboid(
        output,
        pos,
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 0.25],
        head_color,
        None,
        transform,
    );
    cuboid(
        output,
        pos,
        [0.375, 0.375, 0.25],
        [0.625, 0.625, if short { 0.75 } else { 1.0 }],
        [0.56, 0.43, 0.25, 0.0],
        None,
        transform,
    );
}

pub(super) fn rail(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    color: Color,
) {
    let shape = state.property("shape").unwrap_or("north_south");
    let rail_color = active_color(color, state.powered);
    match shape {
        "east_west" => straight_x(output, pos, 0.07, 0.07, rail_color),
        "ascending_east" => straight_x(output, pos, 0.07, 1.07, rail_color),
        "ascending_west" => straight_x(output, pos, 1.07, 0.07, rail_color),
        "ascending_north" => straight_z(output, pos, 1.07, 0.07, rail_color),
        "ascending_south" => straight_z(output, pos, 0.07, 1.07, rail_color),
        "south_east" => curve(output, pos, [0.5, 0.07, 1.0], [1.0, 0.07, 0.5], rail_color),
        "south_west" => curve(output, pos, [0.5, 0.07, 1.0], [0.0, 0.07, 0.5], rail_color),
        "north_west" => curve(output, pos, [0.5, 0.07, 0.0], [0.0, 0.07, 0.5], rail_color),
        "north_east" => curve(output, pos, [0.5, 0.07, 0.0], [1.0, 0.07, 0.5], rail_color),
        _ => straight_z(output, pos, 0.07, 0.07, rail_color),
    }
}

fn straight_x(output: &mut Vec<Vertex>, pos: BlockPos, west_y: f32, east_y: f32, color: Color) {
    for z in [0.25, 0.75] {
        rod(output, pos, [0.0, west_y, z], [1.0, east_y, z], 0.055, color);
    }
    for x in [0.16, 0.5, 0.84] {
        let y = west_y + (east_y - west_y) * x;
        rod(output, pos, [x, y, 0.2], [x, y, 0.8], 0.045, [0.38, 0.27, 0.14, 0.0]);
    }
}

fn straight_z(output: &mut Vec<Vertex>, pos: BlockPos, north_y: f32, south_y: f32, color: Color) {
    for x in [0.25, 0.75] {
        rod(output, pos, [x, north_y, 0.0], [x, south_y, 1.0], 0.055, color);
    }
    for z in [0.16, 0.5, 0.84] {
        let y = north_y + (south_y - north_y) * z;
        rod(output, pos, [0.2, y, z], [0.8, y, z], 0.045, [0.38, 0.27, 0.14, 0.0]);
    }
}

fn curve(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    first: [f32; 3],
    second: [f32; 3],
    color: Color,
) {
    let center = [0.5, 0.07, 0.5];
    rod(output, pos, first, center, 0.07, color);
    rod(output, pos, center, second, 0.07, color);
    let inner_first = [
        first[0] * 0.68 + 0.16,
        first[1],
        first[2] * 0.68 + 0.16,
    ];
    let inner_second = [
        second[0] * 0.68 + 0.16,
        second[1],
        second[2] * 0.68 + 0.16,
    ];
    rod(output, pos, inner_first, center, 0.05, color);
    rod(output, pos, center, inner_second, 0.05, color);
}
