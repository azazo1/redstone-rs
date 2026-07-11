use std::collections::BTreeMap;

use redstone_core::{Action, BlockPos, BlockStateId, Simulation, SimulationConfig, SparseWorld};
use redstone_java_26::{Java26Registry, Java26Rules, StateResolver};

fn state(registry: &mut Java26Registry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    registry
        .resolve_state(
            name,
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
}

fn rail(registry: &mut Java26Registry) -> BlockStateId {
    state(
        registry,
        "minecraft:rail",
        &[("shape", "north_south"), ("waterlogged", "false")],
    )
}

fn powered_rail(registry: &mut Java26Registry) -> BlockStateId {
    state(
        registry,
        "minecraft:powered_rail",
        &[
            ("powered", "false"),
            ("shape", "north_south"),
            ("waterlogged", "false"),
        ],
    )
}

fn slab(registry: &mut Java26Registry, slab_type: &str) -> BlockStateId {
    state(
        registry,
        "minecraft:stone_slab",
        &[("type", slab_type), ("waterlogged", "false")],
    )
}

fn shape(simulation: &Simulation<Java26Rules>, pos: BlockPos) -> &str {
    simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(pos))
        .unwrap()
        .property("shape")
        .unwrap()
}

#[test]
fn rail_pair_repairs_to_an_east_west_line() {
    let mut registry = Java26Registry::new();
    let rail = rail(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, rail).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), rail).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();

    assert_eq!(shape(&simulation, BlockPos::ZERO), "east_west");
    assert_eq!(shape(&simulation, BlockPos::new(1, 0, 0)), "east_west");
}

#[test]
fn ordinary_rail_forms_a_south_east_corner() {
    let mut registry = Java26Registry::new();
    let rail = rail(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [
        BlockPos::ZERO,
        BlockPos::new(1, 0, 0),
        BlockPos::new(0, 0, 1),
    ] {
        world.set_block(pos, rail).unwrap();
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();

    assert_eq!(shape(&simulation, BlockPos::ZERO), "south_east");
}

#[test]
fn lower_rail_ascends_toward_an_upper_neighbor() {
    let mut registry = Java26Registry::new();
    let rail = rail(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, rail).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), rail).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();

    assert_eq!(shape(&simulation, BlockPos::ZERO), "ascending_east");
    assert_eq!(shape(&simulation, BlockPos::new(1, 1, 0)), "east_west");
}

#[test]
fn rail_survives_on_a_top_slab_but_not_a_bottom_slab() {
    for (slab_type, survives) in [("top", true), ("bottom", false)] {
        let mut registry = Java26Registry::new();
        let rail = rail(&mut registry);
        let stone = state(&mut registry, "minecraft:stone", &[]);
        let slab = slab(&mut registry, slab_type);
        let air = registry.air_state();
        let support_pos = BlockPos::ZERO;
        let rail_pos = BlockPos::new(0, 1, 0);
        let mut world = SparseWorld::new(air);
        world.set_block(support_pos, stone).unwrap();
        world.set_block(rail_pos, rail).unwrap();
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default()).unwrap();
        simulation.initialize().unwrap();

        simulation
            .apply(Action::SetBlock {
                pos: support_pos,
                state: slab,
            })
            .unwrap();

        assert_eq!(simulation.world().get_block(rail_pos) != air, survives, "{slab_type}");
    }
}

#[test]
fn straight_rail_types_never_form_a_corner() {
    let mut registry = Java26Registry::new();
    let rail = powered_rail(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [
        BlockPos::ZERO,
        BlockPos::new(1, 0, 0),
        BlockPos::new(0, 0, 1),
    ] {
        world.set_block(pos, rail).unwrap();
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();

    assert!(matches!(
        shape(&simulation, BlockPos::ZERO),
        "north_south" | "east_west"
    ));
}

#[test]
fn powered_three_way_rail_uses_the_vanilla_corner_priority() {
    for (powered, expected) in [(false, "south_east"), (true, "north_east")] {
        let mut registry = Java26Registry::new();
        let rail = rail(&mut registry);
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let mut world = SparseWorld::new(registry.air_state());
        for pos in [
            BlockPos::ZERO,
            BlockPos::new(0, 0, -1),
            BlockPos::new(0, 0, 1),
            BlockPos::new(1, 0, 0),
        ] {
            world.set_block(pos, rail).unwrap();
        }
        if powered {
            world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
        }
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();

        simulation.initialize().unwrap();

        assert_eq!(shape(&simulation, BlockPos::ZERO), expected);
    }
}
