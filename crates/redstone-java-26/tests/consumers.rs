use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockPos, BlockStateId, Direction, Probe, ProbeValue, Simulation, SimulationConfig,
    SparseWorld,
};
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

#[test]
fn a_powered_door_updates_both_halves_and_closes_together() {
    let mut registry = Java26Registry::new();
    let lower = state(
        &mut registry,
        "minecraft:iron_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "left"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    let upper = state(
        &mut registry,
        "minecraft:iron_door",
        &[
            ("facing", "north"),
            ("half", "upper"),
            ("hinge", "left"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let lower_pos = BlockPos::ZERO;
    let upper_pos = BlockPos::new(0, 1, 0);
    let source_pos = BlockPos::new(-1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(lower_pos, lower).unwrap();
    world.set_block(upper_pos, upper).unwrap();
    world.set_block(source_pos, source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();
    for pos in [lower_pos, upper_pos] {
        let state = simulation
            .rules()
            .registry()
            .state(simulation.world().get_block(pos))
            .unwrap();
        assert_eq!(state.property("powered"), Some("true"));
        assert_eq!(state.property("open"), Some("true"));
    }

    simulation
        .apply(Action::BreakBlock { pos: source_pos })
        .unwrap();
    for pos in [lower_pos, upper_pos] {
        let state = simulation
            .rules()
            .registry()
            .state(simulation.world().get_block(pos))
            .unwrap();
        assert_eq!(state.property("powered"), Some("false"));
        assert_eq!(state.property("open"), Some("false"));
    }
}

#[test]
fn powered_rail_signal_stops_after_eight_connected_rails() {
    let mut registry = Java26Registry::new();
    let rail = state(
        &mut registry,
        "minecraft:powered_rail",
        &[
            ("powered", "false"),
            ("shape", "east_west"),
            ("waterlogged", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::new(0, -1, 0), source).unwrap();
    for x in 0..=9 {
        world.set_block(BlockPos::new(x, 0, 0), rail).unwrap();
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();

    for x in 0..=8 {
        let state = simulation
            .rules()
            .registry()
            .state(simulation.world().get_block(BlockPos::new(x, 0, 0)))
            .unwrap();
        assert_eq!(state.property("powered"), Some("true"), "rail {x}");
    }
    let ninth = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::new(9, 0, 0)))
        .unwrap();
    assert_eq!(ninth.property("powered"), Some("false"));
}

#[test]
fn note_block_and_bell_emit_events_only_on_rising_edges() {
    let mut registry = Java26Registry::new();
    let note = state(
        &mut registry,
        "minecraft:note_block",
        &[("instrument", "harp"), ("note", "0"), ("powered", "false")],
    );
    let bell = state(
        &mut registry,
        "minecraft:bell",
        &[
            ("attachment", "floor"),
            ("facing", "north"),
            ("powered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let source_pos = BlockPos::ZERO;
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::new(1, 0, 0), note).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), bell).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "notes",
        Probe::EventCount {
            kind: "note_block_play".to_owned(),
        },
    );
    simulation.add_probe(
        "bells",
        Probe::EventCount {
            kind: "bell_ring".to_owned(),
        },
    );

    let first = simulation
        .step_with_actions(&[Action::SetBlock {
            pos: source_pos,
            state: source,
        }])
        .unwrap();
    assert_eq!(first.probes[0].value, ProbeValue::Integer(1));
    assert_eq!(first.probes[1].value, ProbeValue::Integer(1));
    let held = simulation.step().unwrap();
    assert_eq!(held.probes[0].value, ProbeValue::Integer(1));
    assert_eq!(held.probes[1].value, ProbeValue::Integer(1));

    simulation
        .step_with_actions(&[Action::BreakBlock { pos: source_pos }])
        .unwrap();
    let second = simulation
        .step_with_actions(&[Action::SetBlock {
            pos: source_pos,
            state: source,
        }])
        .unwrap();
    assert_eq!(second.probes[0].value, ProbeValue::Integer(2));
    assert_eq!(second.probes[1].value, ProbeValue::Integer(2));
}

#[test]
fn using_a_note_block_cycles_its_note_and_plays_once() {
    let mut registry = Java26Registry::new();
    let note = state(
        &mut registry,
        "minecraft:note_block",
        &[("instrument", "harp"), ("note", "24"), ("powered", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, note).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "notes",
        Probe::EventCount {
            kind: "note_block_play".to_owned(),
        },
    );

    let delta = simulation
        .step_with_actions(&[Action::UseBlock {
            pos: BlockPos::ZERO,
        }])
        .unwrap();

    let state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(state.property("note"), Some("0"));
    assert_eq!(delta.probes[0].value, ProbeValue::Integer(1));
}

#[test]
fn target_strength_uses_hit_location_and_releases_after_eight_ticks() {
    let mut registry = Java26Registry::new();
    let target = state(&mut registry, "minecraft:target", &[("power", "0")]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, target).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "power",
        Probe::Property {
            pos: BlockPos::ZERO,
            property: "power".to_owned(),
        },
    );

    let hit = simulation
        .step_with_actions(&[Action::HitTarget {
            pos: BlockPos::ZERO,
            face: Direction::North,
            location: [0.9, 0.5, 0.0],
            arrow: false,
        }])
        .unwrap();
    assert_eq!(hit.probes[0].value, ProbeValue::String("3".to_owned()));
    simulation
        .run_until(redstone_core::GameTick(8))
        .unwrap();
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(simulation.world().get_block(BlockPos::ZERO))
            .unwrap()
            .property("power"),
        Some("3")
    );
    let released = simulation.step().unwrap();
    assert_eq!(released.probes[0].value, ProbeValue::String("0".to_owned()));
}
