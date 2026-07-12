use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use redstone_core::{ExecutionMode, Simulation, SimulationConfig};
use redstone_io::{Scenario, StructureLoader, StructureStateResolver};

use crate::{Java26Rules, StateResolveError};
use crate::StateResolver;

use super::*;

#[test]
fn dense_frontend_classifies_runtime_nodes() {
    let cases = [
        (BlockBehavior::RedstoneBlock, Some(NetworkNodeKind::Constant)),
        (BlockBehavior::Torch { wall: false }, Some(NetworkNodeKind::Torch)),
        (BlockBehavior::Repeater, Some(NetworkNodeKind::Repeater)),
        (BlockBehavior::Comparator, Some(NetworkNodeKind::Comparator)),
        (BlockBehavior::Lever, Some(NetworkNodeKind::Lever)),
        (
            BlockBehavior::Button { wooden: false },
            Some(NetworkNodeKind::Button),
        ),
        (
            BlockBehavior::PressurePlate {
                max_weight: None,
                detects_items: false,
            },
            Some(NetworkNodeKind::PressurePlate),
        ),
        (BlockBehavior::Target, Some(NetworkNodeKind::Target)),
        (BlockBehavior::Observer, Some(NetworkNodeKind::DynamicSource)),
        (BlockBehavior::TrappedChest, Some(NetworkNodeKind::DynamicSource)),
        (BlockBehavior::Lamp, Some(NetworkNodeKind::Consumer)),
        (BlockBehavior::Piston { sticky: false }, None),
        (BlockBehavior::Static, None),
    ];
    for (behavior, expected) in cases {
        assert_eq!(identify_kind(&behavior), expected, "{behavior:?}");
    }
}

#[test]
fn wire_search_records_compact_route_and_clamps_weight_fifteen() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lamp).unwrap();
    for x in 1..=3 {
        world.set_block(BlockPos::new(x, 0, 0), wire).unwrap();
    }
    world.set_block(BlockPos::new(4, 0, 0), lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let edge = edge_between(
        &network,
        BlockPos::new(4, 0, 0),
        BlockPos::ZERO,
        NetworkInputKind::Default,
    )
    .unwrap();
    assert_eq!(edge.attenuation, 2);
    assert_eq!(network.node_count(), 2);
    assert_eq!(network.wire_count(), 3);
    assert!(network.node_id(BlockPos::new(2, 0, 0)).is_none());
    let source_wire = network.wire(edge.source_wire.unwrap()).unwrap();
    let target_wire = network.wire(edge.target_wire.unwrap()).unwrap();
    assert_eq!(source_wire.pos, BlockPos::new(3, 0, 0));
    assert_eq!(target_wire.pos, BlockPos::new(1, 0, 0));

    let mut long_world = SparseWorld::new(registry.air_state());
    long_world.set_block(BlockPos::ZERO, lamp).unwrap();
    for x in 1..=16 {
        long_world
            .set_block(BlockPos::new(x, 0, 0), wire)
            .unwrap();
    }
    long_world
        .set_block(BlockPos::new(17, 0, 0), lever)
        .unwrap();
    let long_network = StaticNetwork::compile(&registry, &long_world);
    assert!(
        edge_between(
            &long_network,
            BlockPos::new(17, 0, 0),
            BlockPos::ZERO,
            NetworkInputKind::Default,
        )
        .is_none()
    );
}

#[test]
fn conductor_forwards_only_strong_source_with_actual_path() {
    let mut registry = Java26Registry::new();
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let stone = state_with(&mut registry, "minecraft:stone", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lamp).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), stone).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let edge = edge_between(
        &network,
        BlockPos::new(2, 0, 0),
        BlockPos::ZERO,
        NetworkInputKind::Default,
    )
    .unwrap();
    assert_eq!(edge.attenuation, 0);
    assert_eq!(edge.source_wire, None);
    assert_eq!(edge.target_wire, None);
}

#[test]
fn wire_source_ports_preserve_strong_conductor_rules() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let stone = state_with(&mut registry, "minecraft:stone", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let redstone_block = state_with(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lamp).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), stone).unwrap();
    world.set_block(BlockPos::new(3, 0, 0), lever).unwrap();

    let lever_network = StaticNetwork::compile(&registry, &world);
    assert!(
        edge_between(
            &lever_network,
            BlockPos::new(3, 0, 0),
            BlockPos::ZERO,
            NetworkInputKind::Default,
        )
        .is_some()
    );

    world
        .set_block(BlockPos::new(3, 0, 0), redstone_block)
        .unwrap();
    let block_network = StaticNetwork::compile(&registry, &world);
    assert!(
        edge_between(
            &block_network,
            BlockPos::new(3, 0, 0),
            BlockPos::ZERO,
            NetworkInputKind::Default,
        )
        .is_none()
    );
}

#[test]
fn wire_observer_index_keeps_only_observers_facing_the_wire() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let facing_west = state_with(
        &mut registry,
        "minecraft:observer",
        &[("facing", "west")],
    );
    let wire_pos = BlockPos::ZERO;
    let observing = BlockPos::new(1, 0, 0);
    let facing_away = BlockPos::new(-1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(wire_pos, wire).unwrap();
    world.set_block(observing, facing_west).unwrap();
    world.set_block(facing_away, facing_west).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let wire = network.wire_id(wire_pos).unwrap();
    let observers = network
        .wire_observer_range(wire)
        .map(|index| network.wire_observer(index).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(observers, [observing]);
}

#[test]
fn repeater_side_input_is_typed_and_requires_a_diode() {
    let mut registry = Java26Registry::new();
    let target = state_with(
        &mut registry,
        "minecraft:repeater",
        &[("facing", "north")],
    );
    let source = state_with(
        &mut registry,
        "minecraft:repeater",
        &[("facing", "east")],
    );
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, target).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let edge = edge_between(
        &network,
        BlockPos::new(1, 0, 0),
        BlockPos::ZERO,
        NetworkInputKind::Side,
    )
    .unwrap();
    assert_eq!(edge.attenuation, 0);
    assert!(
        edge_between(
            &network,
            BlockPos::new(1, 0, 0),
            BlockPos::ZERO,
            NetworkInputKind::Default,
        )
        .is_none()
    );
}

#[test]
fn comparator_tracks_far_input_separately_from_default_edges() {
    let mut registry = Java26Registry::new();
    let comparator = state_with(
        &mut registry,
        "minecraft:comparator",
        &[("facing", "north")],
    );
    let stone = state_with(&mut registry, "minecraft:stone", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "north")],
    );
    let comparator_pos = BlockPos::ZERO;
    let rear = BlockPos::new(0, 0, -1);
    let far = BlockPos::new(0, 0, -2);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(comparator_pos, comparator).unwrap();
    world.set_block(rear, stone).unwrap();
    world.set_block(far, lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let comparator_id = network.node_id(comparator_pos).unwrap();
    assert_eq!(
        network.node(comparator_id).unwrap().comparator_far_input,
        Some(far)
    );
    assert!(network.dependents(far).contains(&comparator_id));
    let edge = edge_between(
        &network,
        far,
        comparator_pos,
        NetworkInputKind::Default,
    )
    .unwrap();
    assert_eq!(edge.source_wire, None);
    assert_eq!(edge.target_wire, None);
}

#[test]
fn duplicate_wire_paths_keep_the_lowest_attenuation() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let target = BlockPos::ZERO;
    let source = BlockPos::new(3, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(target, lamp).unwrap();
    for pos in [
        BlockPos::new(1, 0, 0),
        BlockPos::new(2, 0, 0),
        BlockPos::new(0, 0, 1),
        BlockPos::new(1, 0, 1),
        BlockPos::new(2, 0, 1),
        BlockPos::new(3, 0, 1),
    ] {
        world.set_block(pos, wire).unwrap();
    }
    world.set_block(source, lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let source_id = network.node_id(source).unwrap();
    let target_id = network.node_id(target).unwrap();
    let edges = network
        .edges_from(source_id)
        .iter()
        .filter(|edge| edge.target == target_id && edge.kind == NetworkInputKind::Default)
        .collect::<Vec<_>>();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].attenuation, 1);
}

#[test]
fn dense_wire_csr_indexes_sources_targets_and_neighbors() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lamp).unwrap();
    for x in 1..=3 {
        world.set_block(BlockPos::new(x, 0, 0), wire).unwrap();
    }
    world.set_block(BlockPos::new(4, 0, 0), lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let first = network.wire_id(BlockPos::new(1, 0, 0)).unwrap();
    let middle = network.wire_id(BlockPos::new(2, 0, 0)).unwrap();
    let last = network.wire_id(BlockPos::new(3, 0, 0)).unwrap();
    assert_eq!(network.wire_dependents(first), [middle]);
    assert_eq!(network.wire_dependents(middle), [first, last]);
    assert_eq!(network.wire_dependents(last), [middle]);
    assert_eq!(network.wire_predecessors(first), [middle]);
    assert_eq!(network.wire_predecessors(middle), [first, last]);
    assert_eq!(network.wire_predecessors(last), [middle]);
    assert_eq!(network.wire_sources(last).len(), 1);
    assert_eq!(network.wire_targets(first).len(), 1);
    assert!(network.wire_sources(first).is_empty());
    assert!(network.wire_targets(last).is_empty());
}

#[test]
fn downward_wire_step_keeps_power_dependency_directional() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let upper_pos = BlockPos::ZERO;
    let lower_pos = BlockPos::new(1, -1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(upper_pos, wire).unwrap();
    world.set_block(lower_pos, wire).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let upper = network.wire_id(upper_pos).unwrap();
    let lower = network.wire_id(lower_pos).unwrap();
    assert_eq!(network.wire_dependents(lower), [upper]);
    assert!(network.wire_dependents(upper).is_empty());
    assert_eq!(network.wire_predecessors(upper), [lower]);
    assert!(network.wire_predecessors(lower).is_empty());
}

#[test]
fn wire_invalidation_uses_local_csr_closure() {
    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let lamp_pos = BlockPos::ZERO;
    let source_pos = BlockPos::new(4, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(lamp_pos, lamp).unwrap();
    for x in 1..=3 {
        world.set_block(BlockPos::new(x, 0, 0), wire).unwrap();
    }
    world.set_block(source_pos, lever).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let lamp_id = network.node_id(lamp_pos).unwrap();
    assert_eq!(network.affected_nodes(BlockPos::new(2, 0, 0)), [lamp_id]);
    assert_eq!(network.affected_nodes(source_pos), [lamp_id, network.node_id(source_pos).unwrap()]);
}

#[test]
fn long_wire_build_does_not_search_from_every_wire() {
    const WIRE_COUNT: i32 = 4096;

    let mut registry = Java26Registry::new();
    let wire = connected_wire(&mut registry);
    let lamp = state_with(&mut registry, "minecraft:redstone_lamp", &[]);
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "east")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lamp).unwrap();
    for x in 1..=WIRE_COUNT {
        world.set_block(BlockPos::new(x, 0, 0), wire).unwrap();
    }
    world
        .set_block(BlockPos::new(WIRE_COUNT + 1, 0, 0), lever)
        .unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let stats = network.build_stats();
    assert_eq!(network.wire_count(), WIRE_COUNT as usize);
    assert_eq!(stats.wire_neighbor_scans, WIRE_COUNT as usize);
    assert_eq!(stats.wire_neighbor_entries, (WIRE_COUNT as usize - 1) * 2);
    assert_eq!(stats.semantic_wire_searches, 1);
    assert_eq!(stats.semantic_wire_visits, MAX_WIRE_ATTENUATION as usize);
    assert_eq!(stats.wire_source_ports, 1);
    assert_eq!(stats.wire_target_ports, 1);
}

#[test]
fn immutable_inputs_are_folded_without_folding_scheduled_processors() {
    let mut registry = Java26Registry::new();
    let repeater = state_with(
        &mut registry,
        "minecraft:repeater",
        &[("facing", "north")],
    );
    let source = state_with(&mut registry, "minecraft:redstone_block", &[]);
    let target_pos = BlockPos::ZERO;
    let source_pos = BlockPos::new(0, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(target_pos, repeater).unwrap();
    world.set_block(source_pos, source).unwrap();

    let network = StaticNetwork::compile(&registry, &world);
    let target = network.node(network.node_id(target_pos).unwrap()).unwrap();
    assert_eq!(target.constant_output, None);
    assert_eq!(target.constant_input.default, 15);
    assert_eq!(target.constant_input.side, 0);
    assert!(network.input_ports(target.id).is_empty());
    assert!(
        edge_between(
            &network,
            source_pos,
            target_pos,
            NetworkInputKind::Default,
        )
        .is_none()
    );
}

#[test]
#[ignore = "Frostbyte 静态网络内存诊断耗时较长"]
fn frostbyte_hello_world_static_network_diagnostics() {
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenarios/frostbyte-cpu-16bit-hello-world.toml");
    let scenario = Scenario::load(&scenario_path).unwrap();
    let mut resolver = RegistryResolver(Java26Registry::new());
    let base = StructureLoader::load_with_options(
        &scenario.source.path,
        scenario.source.load_options(),
        &mut resolver,
    )
    .unwrap();
    let pastes = scenario
        .source
        .pastes
        .iter()
        .filter(|paste| paste.tick.is_none())
        .map(|paste| {
            let structure = StructureLoader::load_with_options(
                &paste.path,
                paste.load_options(),
                &mut resolver,
            )
            .unwrap();
            (paste, structure)
        })
        .collect::<Vec<_>>();
    let mut simulation = Simulation::load(
        Java26Rules::new(resolver.0),
        base.world,
        SimulationConfig {
            mode: scenario.mode,
            execution_mode: ExecutionMode::Interpreted,
            seed: scenario.seed,
            environment: scenario.environment.into(),
            strict: scenario.strict,
            record_events: false,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    for (paste, structure) in pastes {
        simulation
            .paste_world(
                &structure.world,
                structure.region_min,
                structure.region_max,
                paste.ignore_air,
                paste.paste_entities,
            )
            .unwrap();
        if paste.update {
            simulation
                .update_region(structure.region_min, structure.region_max)
                .unwrap();
        }
    }

    let world_blocks = simulation.world().iter_blocks().count();
    let rss_before = resident_set_bytes();
    let started = Instant::now();
    let network = StaticNetwork::compile(simulation.rules().registry(), simulation.world());
    let elapsed = started.elapsed();
    let rss_after = resident_set_bytes();
    let node_count = network.node_count();
    let wire_count = network.wire_count();
    let edge_count = network.edge_count();
    let reverse_positions = network.reverse_dependencies.len();
    let stats = network.build_stats();
    drop(network);
    let rss_after_drop = resident_set_bytes();
    eprintln!(
        "frostbyte static network: world_blocks={world_blocks} compile_ms={:.3} nodes={node_count} wires={wire_count} edges={edge_count} reverse_positions={reverse_positions} wire_neighbor_entries={} source_ports={} target_ports={} semantic_wire_searches={} semantic_wire_visits={} rss_before={rss_before:?} rss_after={rss_after:?} rss_after_drop={rss_after_drop:?}",
        elapsed.as_secs_f64() * 1_000.0,
        stats.wire_neighbor_entries,
        stats.wire_source_ports,
        stats.wire_target_ports,
        stats.semantic_wire_searches,
        stats.semantic_wire_visits,
    );
}

fn edge_between(
    network: &StaticNetwork,
    source: BlockPos,
    target: BlockPos,
    kind: NetworkInputKind,
) -> Option<&NetworkEdge> {
    let source = network.node_id(source)?;
    let target = network.node_id(target)?;
    network
        .edges_from(source)
        .iter()
        .find(|edge| edge.target == target && edge.kind == kind)
}

fn connected_wire(registry: &mut Java26Registry) -> BlockStateId {
    state_with(
        registry,
        "minecraft:redstone_wire",
        &[
            ("north", "side"),
            ("east", "side"),
            ("south", "side"),
            ("west", "side"),
        ],
    )
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

struct RegistryResolver(Java26Registry);

impl StructureStateResolver for RegistryResolver {
    type Error = StateResolveError;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, Self::Error> {
        self.0.resolve_state(name, properties)
    }

    fn complete_state_properties(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, Self::Error> {
        self.0.complete_state_properties(name, properties)
    }

    fn air_state(&self) -> BlockStateId {
        self.0.air_state()
    }
}

fn resident_set_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then_some(())
        .and_then(|()| std::str::from_utf8(&output.stdout).ok())?
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|kilobytes| kilobytes.checked_mul(1024))
}
