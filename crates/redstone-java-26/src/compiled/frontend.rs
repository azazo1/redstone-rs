use redstone_core::{BlockKindId, BlockPos, BlockStateId, Direction, SparseWorld};
use smallvec::SmallVec;

use crate::{BlockBehavior, Java26Registry, StateDefinition};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum NodeKind {
    ConstantSource,
    Wire,
    Torch,
    Repeater,
    Comparator,
    Lever,
    Button,
    PressurePlate,
    SignalSource,
    Boundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TopologySignature {
    pub kind: BlockKindId,
    pub node_kind: NodeKind,
    pub conductor: bool,
    shape_hash: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InputSource {
    pub(crate) pos: BlockPos,
    pub(crate) direction: Direction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BlockInputPlan {
    pub(crate) pos: BlockPos,
    pub(crate) direction: Direction,
    pub(crate) conductor_sources: SmallVec<[InputSource; 6]>,
    pub(crate) constant_power: u8,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct WirePlan {
    pub(crate) block_inputs: SmallVec<[BlockInputPlan; 6]>,
    pub(crate) wire_inputs: SmallVec<[BlockPos; 12]>,
}

pub(super) fn topology_signature(
    registry: &Java26Registry,
    state_id: BlockStateId,
) -> Option<TopologySignature> {
    let state = registry.state(state_id)?;
    let piston_boundary = matches!(state.behavior, BlockBehavior::Piston { .. })
        || matches!(state.name.as_ref(), "minecraft:moving_piston" | "minecraft:piston_head");
    let node_kind = classify(state).or_else(|| piston_boundary.then_some(NodeKind::Boundary))?;
    let shape_hash = if node_kind == NodeKind::Boundary && !piston_boundary {
        0
    } else {
        shape_hash(state)
    };
    Some(TopologySignature {
        kind: state.kind,
        node_kind,
        conductor: state.redstone_conductor,
        shape_hash,
    })
}

pub(super) fn classify(state: &StateDefinition) -> Option<NodeKind> {
    Some(match state.behavior {
        BlockBehavior::Air => return None,
        BlockBehavior::Static if !state.redstone_conductor => return None,
        BlockBehavior::Static => NodeKind::Boundary,
        BlockBehavior::RedstoneBlock => NodeKind::ConstantSource,
        BlockBehavior::Wire => NodeKind::Wire,
        BlockBehavior::Torch { .. } => NodeKind::Torch,
        BlockBehavior::Repeater => NodeKind::Repeater,
        BlockBehavior::Comparator => NodeKind::Comparator,
        BlockBehavior::Lever => NodeKind::Lever,
        BlockBehavior::Button { .. } => NodeKind::Button,
        BlockBehavior::PressurePlate { .. } => NodeKind::PressurePlate,
        BlockBehavior::Observer
        | BlockBehavior::Target
        | BlockBehavior::TripwireHook
        | BlockBehavior::DetectorRail
        | BlockBehavior::DaylightDetector
        | BlockBehavior::Lectern
        | BlockBehavior::TrappedChest => NodeKind::SignalSource,
        _ => NodeKind::Boundary,
    })
}

pub(super) fn compile_wire_plan(
    registry: &Java26Registry,
    world: &SparseWorld,
    pos: BlockPos,
) -> WirePlan {
    let mut block_inputs = SmallVec::new();
    for direction in Direction::UPDATE_ORDER {
        let input_pos = pos.relative(direction);
        let Some(input_state) = registry.state(world.get_block(input_pos)) else {
            continue;
        };
        let constant_power = if matches!(input_state.behavior, BlockBehavior::RedstoneBlock) {
            15
        } else {
            0
        };
        let mut conductor_sources = SmallVec::new();
        if input_state.redstone_conductor {
            for source_direction in Direction::UPDATE_ORDER {
                let source_pos = input_pos.relative(source_direction);
                let Some(source_state) = registry.state(world.get_block(source_pos)) else {
                    continue;
                };
                if matches!(source_state.behavior, BlockBehavior::Wire) {
                    continue;
                }
                conductor_sources.push(InputSource {
                    pos: source_pos,
                    direction: source_direction,
                });
            }
        }
        block_inputs.push(BlockInputPlan {
            pos: input_pos,
            direction,
            conductor_sources,
            constant_power,
        });
    }
    WirePlan {
        block_inputs,
        wire_inputs: wire_neighbors(registry, world, pos),
    }
}

pub(super) fn wire_neighbors(
    registry: &Java26Registry,
    world: &SparseWorld,
    pos: BlockPos,
) -> SmallVec<[BlockPos; 12]> {
    let above_open = registry
        .state(world.get_block(pos.relative(Direction::Up)))
        .is_none_or(|state| !state.redstone_conductor);
    let mut neighbors = SmallVec::new();
    for horizontal in Direction::HORIZONTAL {
        let side = pos.relative(horizontal);
        let Some(side_state) = registry.state(world.get_block(side)) else {
            continue;
        };
        if matches!(side_state.behavior, BlockBehavior::Wire) {
            neighbors.push(side);
        }
        if side_state.redstone_conductor && above_open {
            let above = side.relative(Direction::Up);
            if registry
                .state(world.get_block(above))
                .is_some_and(|state| matches!(state.behavior, BlockBehavior::Wire))
            {
                neighbors.push(above);
            }
        } else if !side_state.redstone_conductor {
            let below = side.relative(Direction::Down);
            if registry
                .state(world.get_block(below))
                .is_some_and(|state| matches!(state.behavior, BlockBehavior::Wire))
            {
                neighbors.push(below);
            }
        }
    }
    neighbors.sort_unstable();
    neighbors.dedup();
    neighbors
}

pub(super) fn direct_input_positions(
    state: &StateDefinition,
    pos: BlockPos,
    world: &SparseWorld,
    registry: &Java26Registry,
) -> SmallVec<[(BlockPos, u8); 4]> {
    let mut inputs = SmallVec::new();
    match state.behavior {
        BlockBehavior::Torch { wall: true } => {
            if let Some(facing) = state.facing {
                inputs.push((pos.relative(facing.opposite()), 0));
            }
        }
        BlockBehavior::Torch { wall: false } => inputs.push((pos.relative(Direction::Down), 0)),
        BlockBehavior::Repeater | BlockBehavior::Comparator => {
            let facing = state.facing.unwrap_or(Direction::North);
            let rear = pos.relative(facing);
            inputs.push((rear, 0));
            inputs.push((pos.relative(facing.clockwise()), 0));
            inputs.push((pos.relative(facing.counter_clockwise()), 0));
            if matches!(state.behavior, BlockBehavior::Comparator)
                && registry
                    .state(world.get_block(rear))
                    .is_some_and(|state| state.redstone_conductor)
            {
                inputs.push((rear.relative(facing), 0));
            }
        }
        _ => {}
    }
    inputs
}

fn shape_hash(state: &StateDefinition) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for (name, value) in state.properties.iter() {
        if matches!(
            name.as_str(),
            "power" | "powered" | "lit" | "locked" | "enabled" | "triggered"
        ) {
            continue;
        }
        for byte in name.bytes().chain([0]).chain(value.bytes()).chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::StateResolver;

    use super::*;

    #[test]
    fn frontend_classifies_supported_nodes_and_boundaries() {
        let mut registry = Java26Registry::new();
        let cases = [
            ("minecraft:redstone_block", NodeKind::ConstantSource),
            ("minecraft:redstone_wire", NodeKind::Wire),
            ("minecraft:redstone_torch", NodeKind::Torch),
            ("minecraft:repeater", NodeKind::Repeater),
            ("minecraft:comparator", NodeKind::Comparator),
            ("minecraft:lever", NodeKind::Lever),
            ("minecraft:stone_button", NodeKind::Button),
            ("minecraft:stone_pressure_plate", NodeKind::PressurePlate),
            ("minecraft:observer", NodeKind::SignalSource),
            ("minecraft:target", NodeKind::SignalSource),
            ("minecraft:daylight_detector", NodeKind::SignalSource),
            ("minecraft:lectern", NodeKind::SignalSource),
            ("minecraft:trapped_chest", NodeKind::SignalSource),
            ("minecraft:piston", NodeKind::Boundary),
            ("minecraft:redstone_lamp", NodeKind::Boundary),
            ("minecraft:stone", NodeKind::Boundary),
        ];
        for (name, expected) in cases {
            let state = default_state(&mut registry, name);
            assert_eq!(classify(registry.state(state).unwrap()), Some(expected), "{name}");
        }
        let air = default_state(&mut registry, "minecraft:air");
        assert_eq!(classify(registry.state(air).unwrap()), None);
    }

    #[test]
    fn topology_signature_separates_power_from_structure() {
        let mut registry = Java26Registry::new();
        let wire_off = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("power", "0")],
        );
        let wire_on = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("power", "15")],
        );
        assert_eq!(
            topology_signature(&registry, wire_off),
            topology_signature(&registry, wire_on)
        );

        let piston_west = state_with(
            &mut registry,
            "minecraft:piston",
            &[("facing", "west"), ("extended", "false")],
        );
        let piston_east = state_with(
            &mut registry,
            "minecraft:piston",
            &[("facing", "east"), ("extended", "false")],
        );
        assert_ne!(
            topology_signature(&registry, piston_west),
            topology_signature(&registry, piston_east)
        );

        let moving_west = state_with(
            &mut registry,
            "minecraft:moving_piston",
            &[("facing", "west")],
        );
        let moving_east = state_with(
            &mut registry,
            "minecraft:moving_piston",
            &[("facing", "east")],
        );
        assert_ne!(
            topology_signature(&registry, moving_west),
            topology_signature(&registry, moving_east)
        );
    }

    fn default_state(registry: &mut Java26Registry, name: &str) -> BlockStateId {
        state_with(registry, name, &[])
    }

    fn state_with(
        registry: &mut Java26Registry,
        name: &str,
        overrides: &[(&str, &str)],
    ) -> BlockStateId {
        let mut properties = registry
            .complete_state_properties(name, &BTreeMap::new())
            .unwrap();
        for (property, value) in overrides {
            properties.insert((*property).to_owned(), (*value).to_owned());
        }
        registry.resolve_state(name, &properties).unwrap()
    }
}
