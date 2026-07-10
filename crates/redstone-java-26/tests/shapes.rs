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

fn wire(registry: &mut Java26Registry) -> BlockStateId {
    state(
        registry,
        "minecraft:redstone_wire",
        &[
            ("power", "0"),
            ("north", "none"),
            ("east", "none"),
            ("south", "none"),
            ("west", "none"),
        ],
    )
}

#[tokio::test]
async fn isolated_pair_of_wires_becomes_an_east_west_line() {
    let mut registry = Java26Registry::new();
    let wire = wire(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, wire).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.initialize().await.unwrap();

    for pos in [BlockPos::ZERO, BlockPos::new(1, 0, 0)] {
        let state = simulation
            .rules()
            .registry()
            .state(simulation.world().get_block(pos))
            .unwrap();
        assert_eq!(state.property("north"), Some("none"));
        assert_eq!(state.property("east"), Some("side"));
        assert_eq!(state.property("south"), Some("none"));
        assert_eq!(state.property("west"), Some("side"));
    }
}

#[tokio::test]
async fn wire_climbs_a_sturdy_block_to_reach_an_upper_wire() {
    let mut registry = Java26Registry::new();
    let wire = wire(&mut registry);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, wire).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), wire).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.initialize().await.unwrap();

    let state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(state.property("east"), Some("up"));
    assert_eq!(state.property("west"), Some("side"));
}

#[tokio::test]
async fn adding_a_wire_repairs_the_existing_neighbors_shape() {
    let mut registry = Java26Registry::new();
    let wire = wire(&mut registry);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, wire).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation
        .apply(Action::SetBlock {
            pos: BlockPos::new(1, 0, 0),
            state: wire,
        })
        .await
        .unwrap();

    let existing = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(existing.property("east"), Some("side"));
    assert_eq!(existing.property("west"), Some("side"));
}

#[tokio::test]
async fn tripwire_connects_only_to_an_aligned_hook() {
    let mut registry = Java26Registry::new();
    let tripwire = state(
        &mut registry,
        "minecraft:tripwire",
        &[
            ("attached", "false"),
            ("disarmed", "false"),
            ("east", "false"),
            ("north", "false"),
            ("powered", "false"),
            ("south", "false"),
            ("west", "false"),
        ],
    );
    let aligned_hook = state(
        &mut registry,
        "minecraft:tripwire_hook",
        &[
            ("attached", "false"),
            ("facing", "west"),
            ("powered", "false"),
        ],
    );
    let crossed_hook = state(
        &mut registry,
        "minecraft:tripwire_hook",
        &[
            ("attached", "false"),
            ("facing", "east"),
            ("powered", "false"),
        ],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, tripwire).unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), aligned_hook)
        .unwrap();
    world
        .set_block(BlockPos::new(0, 0, 1), crossed_hook)
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.initialize().await.unwrap();

    let state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(state.property("east"), Some("true"));
    assert_eq!(state.property("south"), Some("false"));
}

#[tokio::test]
async fn fence_pane_and_wall_connections_are_repaired() {
    let mut registry = Java26Registry::new();
    let fence = state(
        &mut registry,
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "false"),
        ],
    );
    let gate = state(
        &mut registry,
        "minecraft:oak_fence_gate",
        &[
            ("facing", "north"),
            ("in_wall", "false"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    let pane = state(
        &mut registry,
        "minecraft:glass_pane",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("waterlogged", "false"),
            ("west", "false"),
        ],
    );
    let wall = state(
        &mut registry,
        "minecraft:cobblestone_wall",
        &[
            ("east", "none"),
            ("north", "none"),
            ("south", "none"),
            ("up", "true"),
            ("waterlogged", "false"),
            ("west", "none"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, fence).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), gate).unwrap();
    world.set_block(BlockPos::new(0, 0, 3), pane).unwrap();
    world.set_block(BlockPos::new(1, 0, 3), wall).unwrap();
    world.set_block(BlockPos::new(0, 0, 6), wall).unwrap();
    world.set_block(BlockPos::new(1, 0, 6), wall).unwrap();
    world.set_block(BlockPos::new(-1, 0, 6), wall).unwrap();
    world.set_block(BlockPos::new(0, 1, 6), stone).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.initialize().await.unwrap();

    let fence = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(fence.property("east"), Some("true"));
    let pane = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::new(0, 0, 3)))
        .unwrap();
    assert_eq!(pane.property("east"), Some("true"));
    let wall = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::new(0, 0, 6)))
        .unwrap();
    assert_eq!(wall.property("east"), Some("tall"));
    assert_eq!(wall.property("west"), Some("tall"));
    assert_eq!(wall.property("up"), Some("false"));
}
