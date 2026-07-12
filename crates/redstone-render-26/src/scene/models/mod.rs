mod controls;
mod devices;
mod mechanical;
mod signal;

use redstone_core::{BlockPos, Direction};
use redstone_java_26::{BlockBehavior, StateDefinition};
use redstone_replay_26::RenderTracePiston;

use super::mesh::{Color, Transform, Vertex, cuboid};

pub(crate) fn append_block(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    state: &StateDefinition,
    visible: [bool; 6],
) {
    if state.is_piston_head {
        mechanical::piston_head(output, pos, state, Transform::facing(facing(state)));
        return;
    }
    let color = block_color(state);
    match state.behavior {
        BlockBehavior::Air => {}
        BlockBehavior::Wire => signal::wire(output, pos, state, color),
        BlockBehavior::Torch { wall } => signal::torch(output, pos, state, wall, color),
        BlockBehavior::Repeater => signal::repeater(output, pos, state, color),
        BlockBehavior::Comparator => signal::comparator(output, pos, state, color),
        BlockBehavior::Lever => controls::lever(output, pos, state, color),
        BlockBehavior::Button { wooden } => {
            controls::button(output, pos, state, wooden, color);
        }
        BlockBehavior::PressurePlate { .. } => {
            controls::pressure_plate(output, pos, state, color);
        }
        BlockBehavior::Tripwire => controls::tripwire(output, pos, state, color),
        BlockBehavior::TripwireHook => controls::tripwire_hook(output, pos, state, color),
        BlockBehavior::Door => controls::door(output, pos, state, color),
        BlockBehavior::PoweredConsumer => controls::powered_consumer(output, pos, state, color),
        BlockBehavior::Observer => mechanical::observer(output, pos, state, color),
        BlockBehavior::Piston { sticky } => mechanical::piston(output, pos, state, sticky, color),
        BlockBehavior::PoweredRail | BlockBehavior::DetectorRail => {
            mechanical::rail(output, pos, state, color);
        }
        _ if state.is_rail => mechanical::rail(output, pos, state, color),
        BlockBehavior::Lamp
        | BlockBehavior::CopperBulb
        | BlockBehavior::RedstoneBlock
        | BlockBehavior::Target
        | BlockBehavior::DaylightDetector
        | BlockBehavior::Lectern
        | BlockBehavior::TrappedChest
        | BlockBehavior::Hopper
        | BlockBehavior::Dropper
        | BlockBehavior::Dispenser
        | BlockBehavior::Crafter
        | BlockBehavior::NoteBlock
        | BlockBehavior::Bell
        | BlockBehavior::Tnt => devices::device(output, pos, state, color, visible),
        BlockBehavior::Static
        | BlockBehavior::UnsupportedActive => full_cube(output, pos, color, visible),
    }
}

pub(crate) fn append_moving_piston(
    output: &mut Vec<Vertex>,
    piston: &RenderTracePiston,
    progress: f32,
    moved_state: &StateDefinition,
) {
    let (x, y, z) = piston.direction.step();
    let direction = [x as f32, y as f32, z as f32];
    if piston.source && !piston.extending {
        mechanical::extended_piston(
            output,
            piston.pos,
            moved_state,
            matches!(moved_state.behavior, BlockBehavior::Piston { sticky: true }),
            block_color(moved_state),
        );
        let transform = Transform::facing(piston.direction).translated([
            direction[0] * (1.0 - progress),
            direction[1] * (1.0 - progress),
            direction[2] * (1.0 - progress),
        ]);
        mechanical::piston_head(output, piston.pos, moved_state, transform);
        return;
    }
    let offset = if piston.extending {
        progress - 1.0
    } else {
        1.0 - progress
    };
    let start = output.len();
    append_block(output, piston.pos, moved_state, [true; 6]);
    for vertex in &mut output[start..] {
        vertex.position[0] += direction[0] * offset;
        vertex.position[1] += direction[1] * offset;
        vertex.position[2] += direction[2] * offset;
    }
}

pub(crate) fn block_color(state: &StateDefinition) -> Color {
    let active = state.power > 0 || state.powered || state.lit;
    let emission = if active { 0.72 } else { 0.0 };
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let rgb = match state.behavior {
        BlockBehavior::Wire => if active { [0.95, 0.06, 0.03] } else { [0.32, 0.03, 0.02] },
        BlockBehavior::RedstoneBlock => [0.72, 0.03, 0.03],
        BlockBehavior::Torch { .. } => [0.96, 0.28, 0.08],
        BlockBehavior::Repeater | BlockBehavior::Comparator => [0.83, 0.78, 0.68],
        BlockBehavior::Lamp | BlockBehavior::CopperBulb => if state.lit { [1.0, 0.68, 0.16] } else { [0.35, 0.24, 0.12] },
        BlockBehavior::Piston { sticky: true } => [0.34, 0.58, 0.24],
        BlockBehavior::Piston { sticky: false } => [0.62, 0.48, 0.27],
        BlockBehavior::Lever | BlockBehavior::Button { .. } => [0.58, 0.52, 0.43],
        _ if path.contains("glass") => [0.42, 0.68, 0.72],
        _ if path.contains("copper") => [0.62, 0.42, 0.24],
        _ if path.contains("quartz") => [0.84, 0.82, 0.76],
        _ if path.contains("wood") || path.contains("planks") || path.contains("log") => [0.48, 0.32, 0.16],
        _ if path.contains("stone") || path.contains("deepslate") => [0.42, 0.43, 0.44],
        _ => [0.52, 0.56, 0.58],
    };
    [rgb[0], rgb[1], rgb[2], emission]
}

pub(super) fn full_cube(
    output: &mut Vec<Vertex>,
    pos: BlockPos,
    color: Color,
    visible: [bool; 6],
) {
    cuboid(
        output,
        pos,
        [0.0; 3],
        [1.0; 3],
        color,
        Some(visible),
        Transform::identity(),
    );
}

pub(super) fn facing(state: &StateDefinition) -> Direction {
    state.facing.unwrap_or(Direction::North)
}

pub(super) fn active_color(color: Color, active: bool) -> Color {
    [color[0], color[1], color[2], if active { 0.82 } else { 0.0 }]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use redstone_java_26::{Java26Registry, StateDefinition, StateResolver};

    use super::*;

    #[test]
    fn every_classified_device_has_geometry() {
        let mut registry = Java26Registry::new();
        for name in [
            "minecraft:redstone_wire",
            "minecraft:lever",
            "minecraft:stone_button",
            "minecraft:redstone_block",
            "minecraft:redstone_torch",
            "minecraft:redstone_wall_torch",
            "minecraft:repeater",
            "minecraft:comparator",
            "minecraft:observer",
            "minecraft:piston",
            "minecraft:sticky_piston",
            "minecraft:redstone_lamp",
            "minecraft:copper_bulb",
            "minecraft:target",
            "minecraft:stone_pressure_plate",
            "minecraft:tripwire",
            "minecraft:tripwire_hook",
            "minecraft:detector_rail",
            "minecraft:daylight_detector",
            "minecraft:lectern",
            "minecraft:trapped_chest",
            "minecraft:hopper",
            "minecraft:dropper",
            "minecraft:dispenser",
            "minecraft:crafter",
            "minecraft:tnt",
            "minecraft:oak_door",
            "minecraft:oak_trapdoor",
            "minecraft:oak_fence_gate",
            "minecraft:powered_rail",
            "minecraft:note_block",
            "minecraft:bell",
        ] {
            let state = state(&mut registry, name, []);
            let mut vertices = Vec::new();
            append_block(&mut vertices, BlockPos::ZERO, &state, [true; 6]);
            assert!(!vertices.is_empty(), "{name}");
        }
    }

    #[test]
    fn wire_up_and_ascending_rail_extend_above_flat_models() {
        let mut registry = Java26Registry::new();
        let wire = state(
            &mut registry,
            "minecraft:redstone_wire",
            [("north", "up")],
        );
        let flat_wire = state(
            &mut registry,
            "minecraft:redstone_wire",
            [("north", "side")],
        );
        let ascending = state(
            &mut registry,
            "minecraft:powered_rail",
            [("shape", "ascending_east")],
        );
        let flat = state(
            &mut registry,
            "minecraft:powered_rail",
            [("shape", "east_west")],
        );

        assert!(max_y(&vertices(&wire)) > 0.9);
        assert!(max_y(&vertices(&flat_wire)) < 0.1);
        assert!(max_y(&vertices(&ascending)) > 1.0);
        assert!(max_y(&vertices(&flat)) < 0.2);
    }

    #[test]
    fn door_open_state_rotates_the_thin_axis() {
        let mut registry = Java26Registry::new();
        let closed = state(
            &mut registry,
            "minecraft:oak_door",
            [("facing", "north"), ("open", "false")],
        );
        let open = state(
            &mut registry,
            "minecraft:oak_door",
            [("facing", "north"), ("open", "true"), ("hinge", "left")],
        );
        let closed_bounds = xz_span(&vertices(&closed));
        let open_bounds = xz_span(&vertices(&open));

        assert!(closed_bounds.0 > closed_bounds.1);
        assert!(open_bounds.0 < open_bounds.1);
    }

    #[test]
    fn moving_block_travels_one_block_during_extension() {
        let mut registry = Java26Registry::new();
        let stone = state(&mut registry, "minecraft:stone", []);
        let piston = RenderTracePiston {
            pos: BlockPos::ZERO,
            moved_state: stone.id,
            direction: Direction::East,
            extending: true,
            source: false,
            start_timestamp_ms: 0,
            settle_timestamp_ms: 100,
        };
        let mut start = Vec::new();
        let mut end = Vec::new();
        append_moving_piston(&mut start, &piston, 0.0, &stone);
        append_moving_piston(&mut end, &piston, 1.0, &stone);

        assert_eq!(min_x(&end) - min_x(&start), 1.0);
    }

    fn state<const N: usize>(
        registry: &mut Java26Registry,
        name: &str,
        overrides: [(&str, &str); N],
    ) -> StateDefinition {
        let overrides = overrides
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<BTreeMap<_, _>>();
        let properties = registry
            .complete_state_properties(name, &overrides)
            .unwrap();
        let state_id = registry.resolve_state(name, &properties).unwrap();
        registry.resolve_state_id(state_id).unwrap().clone()
    }

    fn vertices(state: &StateDefinition) -> Vec<Vertex> {
        let mut vertices = Vec::new();
        append_block(&mut vertices, BlockPos::ZERO, state, [true; 6]);
        vertices
    }

    fn max_y(vertices: &[Vertex]) -> f32 {
        vertices
            .iter()
            .map(|vertex| vertex.position[1])
            .fold(f32::NEG_INFINITY, f32::max)
    }

    fn xz_span(vertices: &[Vertex]) -> (f32, f32) {
        let min_x = vertices.iter().map(|vertex| vertex.position[0]).fold(f32::INFINITY, f32::min);
        let max_x = vertices.iter().map(|vertex| vertex.position[0]).fold(f32::NEG_INFINITY, f32::max);
        let min_z = vertices.iter().map(|vertex| vertex.position[2]).fold(f32::INFINITY, f32::min);
        let max_z = vertices.iter().map(|vertex| vertex.position[2]).fold(f32::NEG_INFINITY, f32::max);
        (max_x - min_x, max_z - min_z)
    }

    fn min_x(vertices: &[Vertex]) -> f32 {
        vertices
            .iter()
            .map(|vertex| vertex.position[0])
            .fold(f32::INFINITY, f32::min)
    }
}
