use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockEntityChange, BlockEntityData, BlockPos, BlockStateId, Direction, EntityData,
    Probe, ProbeValue, RedstoneMode, Simulation, SimulationConfig, SparseWorld, TraceKind,
    WorldEvent,
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

fn wire(registry: &mut Java26Registry, power: u8) -> BlockStateId {
    state(
        registry,
        "minecraft:redstone_wire",
        &[
            ("power", &power.to_string()),
            ("north", "side"),
            ("east", "side"),
            ("south", "side"),
            ("west", "side"),
        ],
    )
}

fn container(kind: &str, slots: &[(&str, i64)]) -> BlockEntityData {
    let slot_count = match kind {
        "minecraft:hopper" | "minecraft:brewing_stand" => 5,
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => 3,
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel" => 27,
        _ => 9,
    };
    let inventory = slots
        .iter()
        .enumerate()
        .map(|(slot, (item_id, count))| {
            serde_json::json!({
                "slot": slot,
                "item_id": item_id,
                "count": count,
            })
        })
        .collect::<Vec<_>>();
    BlockEntityData {
        kind: kind.to_owned(),
        fields: BTreeMap::from([
            ("inventory".to_owned(), serde_json::Value::Array(inventory)),
            (
                "item_count".to_owned(),
                serde_json::Value::from(slots.iter().map(|(_, count)| *count).sum::<i64>()),
            ),
            ("slot_count".to_owned(), serde_json::Value::from(slot_count)),
            (
                "capacity".to_owned(),
                serde_json::Value::from(slot_count * 64),
            ),
        ]),
    }
}

fn comparator_output_for_source(
    registry: Java26Registry,
    source: BlockStateId,
) -> ProbeValue {
    comparator_output_for_source_with_data(registry, source, None)
}

fn comparator_output_for_source_with_data(
    mut registry: Java26Registry,
    source: BlockStateId,
    data: Option<BlockEntityData>,
) -> ProbeValue {
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(BlockPos::new(0, 0, -1), source).unwrap();
    if let Some(data) = data {
        world.set_block_entity(BlockPos::new(0, 0, -1), data);
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();
    simulation.step().unwrap();
    simulation.step().unwrap().probes[0].value.clone()
}

#[test]
fn raw_world_load_restores_comparator_output_signal() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "east"),
            ("mode", "compare"),
            ("powered", "true"),
        ],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        BlockEntityData {
            kind: "minecraft:comparator".to_owned(),
            fields: BTreeMap::from([(
                "OutputSignal".to_owned(),
                serde_json::Value::from(9),
            )]),
        },
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default()).unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::East),
        },
    );

    let delta = simulation.step().unwrap();

    assert_eq!(delta.probes[0].value, ProbeValue::Integer(9));
}

#[test]
fn comparator_prioritizes_a_tick_when_its_output_faces_another_diode() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let output_comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "east"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world
        .set_block(BlockPos::new(0, 0, -1), source)
        .unwrap();
    world
        .set_block(BlockPos::new(0, 0, 1), output_comparator)
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default()).unwrap();
    simulation.set_trace_enabled(true);

    simulation.initialize().unwrap();

    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::ScheduledTickQueued {
                pos: BlockPos::ZERO,
                priority: -1,
                ..
            }
        )
    }));
}

#[test]
fn comparator_side_input_ignores_a_strongly_powered_conductor() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "subtract"),
            ("powered", "false"),
        ],
    );
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("delay", "1"),
            ("facing", "north"),
            ("locked", "false"),
            ("powered", "true"),
        ],
    );
    let redstone_block = state(&mut registry, "minecraft:redstone_block", &[]);
    let copper_block = state(&mut registry, "minecraft:waxed_copper_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world
        .set_block(BlockPos::new(0, 0, -1), redstone_block)
        .unwrap();
    world
        .set_block(BlockPos::new(1, 0, -1), repeater)
        .unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), copper_block)
        .unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        BlockEntityData {
            kind: "minecraft:comparator".to_owned(),
            fields: BTreeMap::from([(
                "OutputSignal".to_owned(),
                serde_json::Value::from(0),
            )]),
        },
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default()).unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();

    let deltas = simulation.run_until(redstone_core::GameTick(2)).unwrap();

    assert_eq!(deltas.last().unwrap().probes[0].value, ProbeValue::Integer(15));
}

#[test]
fn redstone_block_powers_a_wire_in_both_modes() {
    for mode in [RedstoneMode::Default, RedstoneMode::Experimental] {
        let mut registry = Java26Registry::new();
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let wire = wire(&mut registry, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, source).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(
            rules,
            world,
            SimulationConfig {
                mode,
                seed: 42,
                ..SimulationConfig::default()
            },
        )
        .unwrap();

        simulation.initialize().unwrap();
        let state_id = simulation.world().get_block(BlockPos::new(1, 0, 0));
        assert_eq!(
            simulation
                .rules()
                .registry()
                .state(state_id)
                .unwrap()
                .property("power"),
            Some("15")
        );
    }
}

#[test]
fn default_wire_does_not_power_itself_through_a_conductor() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let powered_wire = wire(&mut registry, 15);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, source).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), stone).unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), powered_wire)
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation
        .step_with_actions(&[Action::BreakBlock {
            pos: BlockPos::ZERO,
        }])
        .unwrap();

    let wire_id = simulation.world().get_block(BlockPos::new(1, 0, 0));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(wire_id)
            .unwrap()
            .property("power"),
        Some("0")
    );
}

#[test]
fn repeater_waits_for_its_configured_delay_and_outputs_forward() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("facing", "north"),
            ("delay", "1"),
            ("locked", "false"),
            ("powered", "false"),
        ],
    );
    let output_wire = wire(&mut registry, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::new(0, 0, -1), source).unwrap();
    world.set_block(BlockPos::ZERO, repeater).unwrap();
    world
        .set_block(BlockPos::new(0, 0, 1), output_wire)
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();
    simulation.step().unwrap();
    let state_id = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(state_id)
            .unwrap()
            .property("powered"),
        Some("false")
    );

    let delta = simulation.step().unwrap();
    assert_eq!(delta.probes[0].value, ProbeValue::Integer(15),);
    let wire_id = simulation.world().get_block(BlockPos::new(0, 0, 1));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(wire_id)
            .unwrap()
            .property("power"),
        Some("15")
    );
}

#[test]
fn a_side_signal_from_a_non_diode_does_not_lock_a_repeater() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("facing", "east"),
            ("delay", "1"),
            ("locked", "false"),
            ("powered", "false"),
        ],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, repeater).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(0, 0, -1), source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();
    simulation
        .run_until(redstone_core::GameTick(2))
        .unwrap();

    let state_id = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(state_id)
            .unwrap()
            .property("powered"),
        Some("true")
    );
}

#[test]
fn comparator_reads_explicit_block_entity_output_from_a_lectern() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[
            ("facing", "north"),
            ("has_book", "true"),
            ("powered", "false"),
        ],
    );
    let input = BlockPos::new(0, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(input, lectern).unwrap();
    world.set_block_entity(
        input,
        BlockEntityData {
            kind: "minecraft:lectern".to_owned(),
            fields: BTreeMap::from([("comparator_output".to_owned(), serde_json::Value::from(7))]),
        },
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();

    simulation.step().unwrap();
    let delta = simulation.step().unwrap();

    assert_eq!(delta.probes[0].value, ProbeValue::Integer(7));
}

#[test]
fn observer_emits_a_two_tick_pulse_after_observed_change() {
    let mut registry = Java26Registry::new();
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "north"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, observer).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(0, 0, -1),
            state: stone,
        }])
        .unwrap();
    simulation.step().unwrap();
    simulation.step().unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(powered)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    simulation.step().unwrap();
    simulation.step().unwrap();
    let unpowered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(unpowered)
            .unwrap()
            .property("powered"),
        Some("false")
    );
}

#[test]
fn moved_observer_schedules_an_air_update_after_settling() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let moved_pos = BlockPos::new(2, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), observer).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.initialize().unwrap();

    simulation.step().unwrap();
    simulation
        .run_until(redstone_core::GameTick(3))
        .unwrap();

    let moved = simulation.world().get_block(moved_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(moved)
            .unwrap()
            .property("powered"),
        Some("false")
    );
    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            &event.kind,
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                ..
            } if *pos == moved_pos && *trigger_tick == redstone_core::GameTick(5)
        )
    }));

    simulation
        .run_until(redstone_core::GameTick(5))
        .unwrap();
    let moved = simulation.world().get_block(moved_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(moved)
            .unwrap()
            .property("powered"),
        Some("true")
    );
}

#[test]
fn observer_detects_piston_base_extension() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "south"), ("powered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let observer_pos = BlockPos::new(0, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(observer_pos, observer).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.initialize().unwrap();

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(-1, 0, 0),
            state: source,
        }])
        .unwrap();

    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                ..
            } if pos == observer_pos && trigger_tick == redstone_core::GameTick(3)
        )
    }));
}

#[test]
fn observer_detects_piston_base_retraction() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "true"), ("facing", "east")],
    );
    let piston_head = state(
        &mut registry,
        "minecraft:piston_head",
        &[("facing", "east"), ("short", "false"), ("type", "normal")],
    );
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "south"), ("powered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let observer_pos = BlockPos::new(0, 0, -1);
    let source_pos = BlockPos::new(-1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), piston_head)
        .unwrap();
    world.set_block(observer_pos, observer).unwrap();
    world.set_block(source_pos, source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.initialize().unwrap();

    simulation
        .step_with_actions(&[Action::BreakBlock { pos: source_pos }])
        .unwrap();

    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                ..
            } if pos == observer_pos && trigger_tick == redstone_core::GameTick(3)
        )
    }));
}

#[test]
fn observer_detects_piston_head_removal() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "true"), ("facing", "east")],
    );
    let piston_head = state(
        &mut registry,
        "minecraft:piston_head",
        &[("facing", "east"), ("short", "false"), ("type", "normal")],
    );
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "south"), ("powered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let source_pos = BlockPos::new(-1, 0, 0);
    let observer_pos = BlockPos::new(1, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), piston_head)
        .unwrap();
    world.set_block(observer_pos, observer).unwrap();
    world.set_block(source_pos, source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.initialize().unwrap();

    simulation
        .step_with_actions(&[Action::BreakBlock { pos: source_pos }])
        .unwrap();

    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::ScheduledTickQueued {
                pos,
                trigger_tick,
                ..
            } if pos == observer_pos && trigger_tick == redstone_core::GameTick(3)
        )
    }));
}

#[test]
fn piston_destroyed_repeater_notifies_with_the_removed_block_kind() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("delay", "1"),
            ("facing", "east"),
            ("locked", "false"),
            ("powered", "false"),
        ],
    );
    let repeater_kind = registry.state(repeater).unwrap().kind;
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let repeater_pos = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(repeater_pos, repeater).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), stone).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);

    simulation.initialize().unwrap();
    simulation.step().unwrap();

    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::NeighborUpdate {
                source_pos,
                source_block,
                ..
            } if source_pos == repeater_pos && source_block == repeater_kind
        )
    }));
    assert_eq!(
        simulation.world().get_block(BlockPos::new(2, 0, 0)),
        simulation.rules().registry().air_state()
    );
}

#[test]
fn piston_destroying_a_door_repairs_the_other_half() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let lower = state(
        &mut registry,
        "minecraft:oak_door",
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
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "upper"),
            ("hinge", "left"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lower_pos = BlockPos::new(1, 0, 0);
    let upper_pos = BlockPos::new(1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(lower_pos, lower).unwrap();
    world.set_block(upper_pos, upper).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), stone).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();
    simulation.step().unwrap();

    assert_eq!(
        simulation.world().get_block(upper_pos),
        simulation.rules().registry().air_state()
    );
}

#[test]
fn sticky_piston_does_not_pull_glazed_terracotta() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:sticky_piston",
        &[("extended", "true"), ("facing", "east")],
    );
    let piston_head = state(
        &mut registry,
        "minecraft:piston_head",
        &[("facing", "east"), ("short", "false"), ("type", "sticky")],
    );
    let glazed = state(
        &mut registry,
        "minecraft:white_glazed_terracotta",
        &[("facing", "north")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let source_pos = BlockPos::new(-1, 0, 0);
    let glazed_pos = BlockPos::new(2, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), piston_head)
        .unwrap();
    world.set_block(glazed_pos, glazed).unwrap();
    world.set_block(source_pos, source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation
        .step_with_actions(&[Action::BreakBlock { pos: source_pos }])
        .unwrap();

    assert_eq!(simulation.world().get_block(glazed_pos), glazed);
}

#[test]
fn powered_observer_resets_when_placed() {
    let mut registry = Java26Registry::new();
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "true")],
    );
    let world = SparseWorld::new(registry.air_state());
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::ZERO,
            state: observer,
        }])
        .unwrap();

    let placed = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(placed)
            .unwrap()
            .property("powered"),
        Some("false")
    );
    assert_eq!(simulation.pending_scheduled_ticks(), 0);
}

#[test]
fn removing_active_observer_refreshes_output_neighbors() {
    let mut registry = Java26Registry::new();
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let output_pos = BlockPos::new(-1, 0, 0);
    let lamp_pos = BlockPos::new(-1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, observer).unwrap();
    world.set_block(output_pos, stone).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(1, 0, 0),
            state: stone,
        }])
        .unwrap();
    simulation
        .run_until(redstone_core::GameTick(3))
        .unwrap();
    let active = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(active)
            .unwrap()
            .property("powered"),
        Some("true")
    );

    let trace_len = simulation.trace().events().len();
    simulation
        .apply(Action::BreakBlock {
            pos: BlockPos::ZERO,
        })
        .unwrap();
    let trace = simulation.trace();
    assert!(trace.events()[trace_len..].iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::NeighborUpdate {
                pos,
                source_pos,
                ..
            } if pos == lamp_pos && source_pos == output_pos
        )
    }));
}

#[test]
fn observers_detect_triggered_containers_and_observer_state_changes() {
    let mut registry = Java26Registry::new();
    let dropper = state(
        &mut registry,
        "minecraft:dropper",
        &[("facing", "north"), ("triggered", "false")],
    );
    let vertical_observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "down"), ("powered", "false")],
    );
    let horizontal_observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "west"), ("powered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let dropper_pos = BlockPos::ZERO;
    let vertical_pos = BlockPos::new(0, 1, 0);
    let horizontal_pos = BlockPos::new(1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(dropper_pos, dropper).unwrap();
    world.set_block(vertical_pos, vertical_observer).unwrap();
    world
        .set_block(horizontal_pos, horizontal_observer)
        .unwrap();
    world.set_block_entity(dropper_pos, container("minecraft:dropper", &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();
    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(-1, 0, 0),
            state: source,
        }])
        .unwrap();
    simulation
        .run_until(redstone_core::GameTick(5))
        .unwrap();

    let vertical = simulation.world().get_block(vertical_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(vertical)
            .unwrap()
            .property("powered"),
        Some("false")
    );
    let horizontal = simulation.world().get_block(horizontal_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(horizontal)
            .unwrap()
            .property("powered"),
        Some("true")
    );
}

#[test]
fn observer_ignores_secondary_updates_from_an_unchanged_conductor() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let wire = wire(&mut registry, 0);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let observer_pos = BlockPos::new(0, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(observer_pos, observer).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), source).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.initialize().unwrap();
    simulation
        .run_until(redstone_core::GameTick(2))
        .unwrap();

    let observer = simulation.world().get_block(observer_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(observer)
            .unwrap()
            .property("powered"),
        Some("false")
    );
}

#[test]
fn observer_powers_a_quasi_connected_piston_through_slime() {
    let mut registry = Java26Registry::new();
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "west"), ("powered", "false")],
    );
    let slime = state(&mut registry, "minecraft:slime_block", &[]);
    let piston = state(
        &mut registry,
        "minecraft:sticky_piston",
        &[("extended", "false"), ("facing", "north")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let observer_pos = BlockPos::ZERO;
    let piston_pos = BlockPos::new(1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(observer_pos, observer).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), slime).unwrap();
    world.set_block(piston_pos, piston).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.add_probe(
        "slime_signal",
        Probe::Signal {
            pos: BlockPos::new(1, 0, 0),
            direction: Some(Direction::Down),
        },
    );

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(-1, 0, 0),
            state: stone,
        }])
        .unwrap();
    let deltas = simulation
        .run_until(redstone_core::GameTick(3))
        .unwrap();

    let observer = simulation.world().get_block(observer_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(observer)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::NeighborUpdate { pos, .. } if pos == piston_pos
        )
    }));
    assert_eq!(
        deltas.last().unwrap().probes[0].value,
        ProbeValue::Integer(15)
    );
    let piston = simulation.world().get_block(piston_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(piston)
            .unwrap()
            .property("extended"),
        Some("true")
    );
}

#[test]
fn hopper_pulls_one_item_and_starts_an_eight_tick_cooldown() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "north"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let source_pos = hopper_pos.relative(Direction::Up);
    let target_pos = hopper_pos.relative(Direction::East);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(source_pos, chest).unwrap();
    world.set_block(target_pos, chest).unwrap();
    world.set_block_entity(hopper_pos, container("minecraft:hopper", &[]));
    world.set_block_entity(
        source_pos,
        container("minecraft:chest", &[("minecraft:stone", 2)]),
    );
    world.set_block_entity(target_pos, container("minecraft:chest", &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    let delta = simulation.step().unwrap();

    assert_eq!(
        simulation.world().block_entity(hopper_pos).unwrap().fields["item_count"],
        1
    );
    assert_eq!(
        simulation.world().block_entity(source_pos).unwrap().fields["item_count"],
        1
    );
    assert_eq!(
        simulation.world().block_entity(target_pos).unwrap().fields["item_count"],
        0
    );
    assert_eq!(
        simulation.world().block_entity(hopper_pos).unwrap().fields["cooldown"],
        8
    );
    let updates = delta
        .events
        .iter()
        .filter_map(|event| match event {
            WorldEvent::BlockEntity {
                change: BlockEntityChange::Update { pos, new_data, .. },
            } => Some((*pos, new_data)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        updates.iter().map(|(pos, _)| *pos).collect::<Vec<_>>(),
        [hopper_pos, source_pos, hopper_pos]
    );
    assert_eq!(updates[0].1.fields["item_count"], 1);
    assert_eq!(updates[1].1.fields["item_count"], 1);
    assert_eq!(updates[2].1.fields["cooldown"], 8);
}

#[test]
fn powered_dropper_transfers_into_the_facing_container_after_four_ticks() {
    let mut registry = Java26Registry::new();
    let dropper = state(
        &mut registry,
        "minecraft:dropper",
        &[("facing", "east"), ("triggered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "north"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let target = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, dropper).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    world.set_block(target, chest).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        container("minecraft:dropper", &[("minecraft:redstone", 2)]),
    );
    world.set_block_entity(target, container("minecraft:chest", &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation
        .run_until(redstone_core::GameTick(5))
        .unwrap();

    assert_eq!(
        simulation
            .world()
            .block_entity(BlockPos::ZERO)
            .unwrap()
            .fields["item_count"],
        1
    );
    assert_eq!(
        simulation.world().block_entity(target).unwrap().fields["item_count"],
        1
    );
}

#[test]
fn dispenser_consumes_registered_items_but_preserves_unknown_items() {
    for (item_id, consumed, entity_kind) in [
        ("minecraft:arrow", true, Some("minecraft:arrow")),
        ("minecraft:stone", false, None),
    ] {
        let mut registry = Java26Registry::new();
        let dispenser = state(
            &mut registry,
            "minecraft:dispenser",
            &[("facing", "east"), ("triggered", "false")],
        );
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, dispenser).unwrap();
        world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
        world.set_block_entity(
            BlockPos::ZERO,
            container("minecraft:dispenser", &[(item_id, 1)]),
        );
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();
        simulation.set_trace_enabled(true);
        simulation.initialize().unwrap();

        simulation
            .run_until(redstone_core::GameTick(5))
            .unwrap();

        assert_eq!(
            simulation
                .world()
                .block_entity(BlockPos::ZERO)
                .unwrap()
                .fields["item_count"],
            i64::from(!consumed)
        );
        assert_eq!(
            simulation
                .world()
                .entities()
                .find(|(_, entity)| entity.kind == entity_kind.unwrap_or_default())
                .map(|(_, entity)| entity.kind.as_str()),
            entity_kind
        );
        assert_eq!(
            simulation
                .trace()
                .events()
                .iter()
                .any(|event| matches!(event.kind, TraceKind::UnsupportedTrigger { .. })),
            !consumed
        );
    }
}

#[test]
fn crafter_emits_output_and_clears_its_crafting_pulse() {
    let mut registry = Java26Registry::new();
    let crafter = state(
        &mut registry,
        "minecraft:crafter",
        &[
            ("crafting", "false"),
            ("orientation", "east_up"),
            ("triggered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, crafter).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    let mut data = container("minecraft:crafter", &[("minecraft:stone", 1)]);
    data.fields.insert(
        "output_item_id".to_owned(),
        serde_json::Value::String("minecraft:redstone".to_owned()),
    );
    data.fields
        .insert("output_count".to_owned(), serde_json::Value::from(2));
    world.set_block_entity(BlockPos::ZERO, data);
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation
        .run_until(redstone_core::GameTick(6))
        .unwrap();

    assert_eq!(
        simulation
            .world()
            .block_entity(BlockPos::ZERO)
            .unwrap()
            .fields["item_count"],
        0
    );
    assert_eq!(simulation.world().entities().count(), 1);
    let output = simulation.world().entities().next().unwrap().1;
    assert_eq!(output.fields["item_id"], "minecraft:redstone");
    assert_eq!(output.fields["item_count"], 2);
    let final_state = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(final_state)
            .unwrap()
            .property("crafting"),
        Some("false")
    );
}

#[test]
fn floor_button_notifies_consumers_next_to_its_strongly_powered_support() {
    let mut registry = Java26Registry::new();
    let button = state(
        &mut registry,
        "minecraft:oak_button",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let button_pos = BlockPos::new(0, 1, 0);
    let lamp_pos = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(button_pos, button).unwrap();
    world.set_block(BlockPos::ZERO, stone).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation
        .step_with_actions(&[Action::PressButton { pos: button_pos }])
        .unwrap();

    let lit = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit)
            .unwrap()
            .property("lit"),
        Some("true")
    );
    simulation
        .run_until(redstone_core::GameTick(35))
        .unwrap();
    let unlit = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(unlit)
            .unwrap()
            .property("lit"),
        Some("false")
    );
}

#[test]
fn floor_sources_strongly_power_the_block_below() {
    let mut registry = Java26Registry::new();
    let pressure_plate = state(
        &mut registry,
        "minecraft:oak_pressure_plate",
        &[("powered", "true")],
    );
    let detector_rail = state(
        &mut registry,
        "minecraft:detector_rail",
        &[
            ("powered", "true"),
            ("shape", "north_south"),
            ("waterlogged", "false"),
        ],
    );
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[
            ("facing", "north"),
            ("has_book", "true"),
            ("powered", "true"),
        ],
    );
    let trapped_chest = state(
        &mut registry,
        "minecraft:trapped_chest",
        &[
            ("facing", "north"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let trapped_chest_data = BlockEntityData {
        kind: "minecraft:trapped_chest".to_owned(),
        fields: BTreeMap::from([("open_count".to_owned(), serde_json::Value::from(1))]),
    };

    for (source, block_entity) in [
        (pressure_plate, None),
        (detector_rail, None),
        (lectern, None),
        (trapped_chest, Some(trapped_chest_data)),
    ] {
        let source_pos = BlockPos::new(0, 1, 0);
        let lamp_pos = BlockPos::new(1, 0, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(source_pos, source).unwrap();
        world.set_block(BlockPos::ZERO, stone).unwrap();
        world.set_block(lamp_pos, lamp).unwrap();
        if let Some(data) = block_entity {
            world.set_block_entity(source_pos, data);
        }
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();

        simulation.initialize().unwrap();

        let lit = simulation.world().get_block(lamp_pos);
        assert_eq!(
            simulation
                .rules()
                .registry()
                .state(lit)
                .unwrap()
                .property("lit"),
            Some("true")
        );
    }
}

#[test]
fn floor_torch_notifies_consumers_around_its_strongly_powered_block() {
    let mut registry = Java26Registry::new();
    let lever = state(
        &mut registry,
        "minecraft:lever",
        &[("face", "wall"), ("facing", "north"), ("powered", "true")],
    );
    let torch = state(
        &mut registry,
        "minecraft:redstone_torch",
        &[("lit", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let lever_pos = BlockPos::new(0, 0, -1);
    let support_pos = BlockPos::ZERO;
    let torch_pos = BlockPos::new(0, 1, 0);
    let powered_block_pos = BlockPos::new(0, 2, 0);
    let lamp_pos = BlockPos::new(1, 2, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(lever_pos, lever).unwrap();
    world.set_block(support_pos, stone).unwrap();
    world.set_block(torch_pos, torch).unwrap();
    world.set_block(powered_block_pos, stone).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation
        .step_with_actions(&[Action::PullLever { pos: lever_pos }])
        .unwrap();
    simulation
        .run_until(redstone_core::GameTick(4))
        .unwrap();

    let lit_torch = simulation.world().get_block(torch_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit_torch)
            .unwrap()
            .property("lit"),
        Some("true")
    );
    let lit_lamp = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit_lamp)
            .unwrap()
            .property("lit"),
        Some("true")
    );
}

#[test]
fn copper_bulb_updates_a_comparator_and_its_output_conductor() {
    let mut registry = Java26Registry::new();
    let lever = state(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let copper_bulb = state(
        &mut registry,
        "minecraft:copper_bulb",
        &[("lit", "false"), ("powered", "false")],
    );
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "west"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let bulb_pos = BlockPos::ZERO;
    let lever_pos = BlockPos::new(0, 1, 0);
    let comparator_pos = BlockPos::new(1, 0, 0);
    let output_block_pos = BlockPos::new(2, 0, 0);
    let lamp_pos = BlockPos::new(3, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(bulb_pos, copper_bulb).unwrap();
    world.set_block(lever_pos, lever).unwrap();
    world.set_block(comparator_pos, comparator).unwrap();
    world.set_block(output_block_pos, stone).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation
        .step_with_actions(&[Action::PullLever { pos: lever_pos }])
        .unwrap();
    simulation
        .run_until(redstone_core::GameTick(3))
        .unwrap();

    let lit_bulb = simulation.world().get_block(bulb_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit_bulb)
            .unwrap()
            .property("lit"),
        Some("true")
    );
    let powered_comparator = simulation.world().get_block(comparator_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(powered_comparator)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    let lit_lamp = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit_lamp)
            .unwrap()
            .property("lit"),
        Some("true")
    );
}

#[test]
fn state_backed_analog_sources_drive_comparators() {
    let mut registry = Java26Registry::new();
    let sources = [
        (
            state(&mut registry, "minecraft:water_cauldron", &[("level", "3")]),
            3,
        ),
        (state(&mut registry, "minecraft:lava_cauldron", &[]), 3),
        (
            state(
                &mut registry,
                "minecraft:powder_snow_cauldron",
                &[("level", "2")],
            ),
            2,
        ),
        (
            state(&mut registry, "minecraft:composter", &[("level", "8")]),
            8,
        ),
        (state(&mut registry, "minecraft:cake", &[("bites", "6")]), 2),
        (
            state(
                &mut registry,
                "minecraft:beehive",
                &[("facing", "north"), ("honey_level", "5")],
            ),
            5,
        ),
        (
            state(
                &mut registry,
                "minecraft:end_portal_frame",
                &[("eye", "true"), ("facing", "north")],
            ),
            15,
        ),
        (
            state(
                &mut registry,
                "minecraft:respawn_anchor",
                &[("charges", "3")],
            ),
            11,
        ),
        (
            state(
                &mut registry,
                "minecraft:copper_golem_statue",
                &[
                    ("copper_golem_pose", "star"),
                    ("facing", "north"),
                    ("waterlogged", "false"),
                ],
            ),
            4,
        ),
        (
            state(
                &mut registry,
                "minecraft:waxed_oxidized_copper_bulb",
                &[("lit", "true"), ("powered", "false")],
            ),
            15,
        ),
        (
            state(
                &mut registry,
                "minecraft:red_candle_cake",
                &[("lit", "false")],
            ),
            14,
        ),
    ];

    for (source, expected) in sources {
        assert_eq!(
            comparator_output_for_source(registry.clone(), source),
            ProbeValue::Integer(expected)
        );
    }
}

#[test]
fn block_entity_backed_analog_sources_drive_comparators() {
    let mut registry = Java26Registry::new();
    let sources = [
        (
            state(
                &mut registry,
                "minecraft:lectern",
                &[
                    ("facing", "north"),
                    ("has_book", "true"),
                    ("powered", "false"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:lectern".to_owned(),
                fields: BTreeMap::from([
                    ("Page".to_owned(), serde_json::Value::from(2)),
                    (
                        "Book".to_owned(),
                        serde_json::json!({
                            "id": "minecraft:written_book",
                            "count": 1,
                            "components": {
                                "minecraft:written_book_content": {
                                    "pages": ["a", "b", "c", "d", "e"]
                                }
                            }
                        }),
                    ),
                ]),
            },
            8,
        ),
        (
            state(
                &mut registry,
                "minecraft:jukebox",
                &[("has_record", "true")],
            ),
            BlockEntityData {
                kind: "minecraft:jukebox".to_owned(),
                fields: BTreeMap::from([(
                    "RecordItem".to_owned(),
                    serde_json::json!({
                        "id": "minecraft:music_disc_5",
                        "count": 1,
                    }),
                )]),
            },
            15,
        ),
        (
            state(
                &mut registry,
                "minecraft:decorated_pot",
                &[
                    ("cracked", "false"),
                    ("facing", "north"),
                    ("waterlogged", "false"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:decorated_pot".to_owned(),
                fields: BTreeMap::from([(
                    "item".to_owned(),
                    serde_json::json!({
                        "id": "minecraft:diamond_sword",
                        "count": 1,
                    }),
                )]),
            },
            15,
        ),
        (
            state(
                &mut registry,
                "minecraft:chiseled_bookshelf",
                &[
                    ("facing", "north"),
                    ("slot_0_occupied", "false"),
                    ("slot_1_occupied", "true"),
                    ("slot_2_occupied", "false"),
                    ("slot_3_occupied", "false"),
                    ("slot_4_occupied", "false"),
                    ("slot_5_occupied", "false"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:chiseled_bookshelf".to_owned(),
                fields: BTreeMap::from([(
                    "last_interacted_slot".to_owned(),
                    serde_json::Value::from(1),
                )]),
            },
            2,
        ),
    ];

    for (source, data, expected) in sources {
        assert_eq!(
            comparator_output_for_source_with_data(registry.clone(), source, Some(data)),
            ProbeValue::Integer(expected)
        );
    }
}

#[test]
fn comparator_reads_combined_copper_chest_inventory() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let left = state(
        &mut registry,
        "minecraft:copper_chest",
        &[
            ("facing", "north"),
            ("type", "left"),
            ("waterlogged", "false"),
        ],
    );
    let right = state(
        &mut registry,
        "minecraft:exposed_copper_chest",
        &[
            ("facing", "north"),
            ("type", "right"),
            ("waterlogged", "false"),
        ],
    );
    let source_pos = BlockPos::new(0, 0, -1);
    let partner_pos = BlockPos::new(1, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(source_pos, left).unwrap();
    world.set_block(partner_pos, right).unwrap();
    world.set_block_entity(source_pos, container("minecraft:chest", &[]));
    world.set_block_entity(
        partner_pos,
        container("minecraft:chest", &[("minecraft:stone", 64)]),
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();
    simulation.step().unwrap();
    let delta = simulation.step().unwrap();

    assert_eq!(delta.probes[0].value, ProbeValue::Integer(1));
}

#[test]
fn remaining_analog_source_categories_drive_comparators() {
    let mut registry = Java26Registry::new();
    let sources = [
        (
            state(
                &mut registry,
                "minecraft:furnace",
                &[("facing", "north"), ("lit", "false")],
            ),
            container("minecraft:furnace", &[("minecraft:diamond_sword", 1)]),
            5,
        ),
        (
            state(
                &mut registry,
                "minecraft:crafter",
                &[
                    ("crafting", "false"),
                    ("orientation", "north_up"),
                    ("triggered", "false"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:crafter".to_owned(),
                fields: BTreeMap::from([
                    ("inventory".to_owned(), serde_json::Value::Array(Vec::new())),
                    ("disabled_slots".to_owned(), serde_json::json!([0, 4, 8])),
                ]),
            },
            3,
        ),
        (
            state(
                &mut registry,
                "minecraft:command_block",
                &[("conditional", "false"), ("facing", "north")],
            ),
            BlockEntityData {
                kind: "minecraft:command_block".to_owned(),
                fields: BTreeMap::from([("SuccessCount".to_owned(), serde_json::Value::from(9))]),
            },
            9,
        ),
        (
            state(
                &mut registry,
                "minecraft:sculk_sensor",
                &[
                    ("power", "4"),
                    ("sculk_sensor_phase", "active"),
                    ("waterlogged", "false"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:sculk_sensor".to_owned(),
                fields: BTreeMap::from([(
                    "last_vibration_frequency".to_owned(),
                    serde_json::Value::from(12),
                )]),
            },
            12,
        ),
        (
            state(
                &mut registry,
                "minecraft:creaking_heart",
                &[
                    ("axis", "y"),
                    ("creaking_heart_state", "awake"),
                    ("natural", "true"),
                ],
            ),
            BlockEntityData {
                kind: "minecraft:creaking_heart".to_owned(),
                fields: BTreeMap::from([("output_signal".to_owned(), serde_json::Value::from(6))]),
            },
            6,
        ),
    ];

    for (source, data, expected) in sources {
        assert_eq!(
            comparator_output_for_source_with_data(registry.clone(), source, Some(data)),
            ProbeValue::Integer(expected)
        );
    }
}

#[test]
fn shelf_output_depends_on_the_comparator_side() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let shelf = state(
        &mut registry,
        "minecraft:oak_shelf",
        &[
            ("facing", "north"),
            ("powered", "false"),
            ("side_chain", "unconnected"),
            ("waterlogged", "false"),
        ],
    );
    let source_pos = BlockPos::new(0, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(source_pos, shelf).unwrap();
    world.set_block_entity(
        source_pos,
        BlockEntityData {
            kind: "minecraft:shelf".to_owned(),
            fields: BTreeMap::from([(
                "inventory".to_owned(),
                serde_json::json!([
                    {"slot": 0, "item_id": "minecraft:stone", "count": 1},
                    {"slot": 2, "item_id": "minecraft:stone", "count": 1}
                ]),
            )]),
        },
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().unwrap();
    simulation.step().unwrap();
    let delta = simulation.step().unwrap();

    assert_eq!(delta.probes[0].value, ProbeValue::Integer(5));
}

#[test]
fn detector_rail_reads_command_and_container_minecarts() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let detector = state(
        &mut registry,
        "minecraft:detector_rail",
        &[
            ("powered", "true"),
            ("shape", "north_south"),
            ("waterlogged", "false"),
        ],
    );
    let entities = [
        (
            EntityData {
                kind: "minecraft:command_block_minecart".to_owned(),
                position: [0.5, 0.1, -0.5],
                fields: BTreeMap::from([("SuccessCount".to_owned(), serde_json::Value::from(9))]),
            },
            9,
        ),
        (
            EntityData {
                kind: "minecraft:chest_minecart".to_owned(),
                position: [0.5, 0.1, -0.5],
                fields: BTreeMap::from([
                    (
                        "inventory".to_owned(),
                        serde_json::json!([
                            {"slot": 0, "item_id": "minecraft:diamond_sword", "count": 1}
                        ]),
                    ),
                    ("slot_count".to_owned(), serde_json::Value::from(27)),
                    ("capacity".to_owned(), serde_json::Value::from(1728)),
                ]),
            },
            1,
        ),
    ];

    for (entity, expected) in entities {
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, comparator).unwrap();
        world.set_block(BlockPos::new(0, 0, -1), detector).unwrap();
        world.spawn_entity(entity);
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();
        simulation.add_probe(
            "output",
            Probe::Signal {
                pos: BlockPos::ZERO,
                direction: Some(Direction::North),
            },
        );
        simulation.initialize().unwrap();
        simulation.step().unwrap();
        let delta = simulation.step().unwrap();

        assert_eq!(delta.probes[0].value, ProbeValue::Integer(expected));
    }
}

#[test]
fn comparator_refreshes_when_a_direct_or_blocked_source_changes() {
    for blocked in [false, true] {
        let mut registry = Java26Registry::new();
        let comparator = state(
            &mut registry,
            "minecraft:comparator",
            &[
                ("facing", "north"),
                ("mode", "compare"),
                ("powered", "false"),
            ],
        );
        let empty = state(&mut registry, "minecraft:cauldron", &[]);
        let full = state(&mut registry, "minecraft:water_cauldron", &[("level", "3")]);
        let stone = state(&mut registry, "minecraft:stone", &[]);
        let comparator_pos = BlockPos::ZERO;
        let source_pos = BlockPos::new(0, 0, if blocked { -2 } else { -1 });
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(comparator_pos, comparator).unwrap();
        world.set_block(source_pos, empty).unwrap();
        if blocked {
            world.set_block(BlockPos::new(0, 0, -1), stone).unwrap();
        }
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();
        simulation.add_probe(
            "output",
            Probe::Signal {
                pos: comparator_pos,
                direction: Some(Direction::North),
            },
        );
        simulation.initialize().unwrap();
        simulation
            .run_until(redstone_core::GameTick(3))
            .unwrap();

        simulation
            .step_with_actions(&[Action::SetBlock {
                pos: source_pos,
                state: full,
            }])
            .unwrap();
        simulation.step().unwrap();
        let delta = simulation.step().unwrap();

        assert_eq!(delta.probes[0].value, ProbeValue::Integer(3));
    }
}

#[test]
fn comparator_refreshes_after_a_hopper_transfer() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "west"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "north"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let source_pos = BlockPos::new(0, 1, 0);
    let comparator_pos = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(source_pos, chest).unwrap();
    world.set_block(comparator_pos, comparator).unwrap();
    world.set_block_entity(hopper_pos, container("minecraft:hopper", &[]));
    world.set_block_entity(
        source_pos,
        container("minecraft:chest", &[("minecraft:stone", 1)]),
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: comparator_pos,
            direction: Some(Direction::West),
        },
    );
    simulation.initialize().unwrap();
    let deltas = simulation
        .run_until(redstone_core::GameTick(3))
        .unwrap();

    assert_eq!(
        deltas.last().unwrap().probes[0].value,
        ProbeValue::Integer(1)
    );
}

#[test]
fn weak_only_sources_and_lit_copper_bulbs_do_not_power_through_a_conductor() {
    let mut registry = Java26Registry::new();
    let redstone_block = state(&mut registry, "minecraft:redstone_block", &[]);
    let target = state(&mut registry, "minecraft:target", &[("power", "15")]);
    let daylight_detector = state(
        &mut registry,
        "minecraft:daylight_detector",
        &[("inverted", "false"), ("power", "15")],
    );
    let copper_bulb = state(
        &mut registry,
        "minecraft:copper_bulb",
        &[("lit", "true"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );

    for source in [redstone_block, target, daylight_detector, copper_bulb] {
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
        world.set_block(BlockPos::ZERO, stone).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), lamp).unwrap();
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();

        simulation.initialize().unwrap();

        let unlit = simulation.world().get_block(BlockPos::new(1, 0, 0));
        assert_eq!(
            simulation
                .rules()
                .registry()
                .state(unlit)
                .unwrap()
                .property("lit"),
            Some("false")
        );
    }
}

#[test]
fn pistons_and_hoppers_do_not_relay_strong_power() {
    let mut registry = Java26Registry::new();
    let source = state(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "true")],
    );
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "up")],
    );
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let redstone_lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );

    for machine in [piston, hopper] {
        let output_pos = BlockPos::new(2, 0, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(1, 1, 0), source).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), machine).unwrap();
        world.set_block(output_pos, redstone_lamp).unwrap();
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();

        simulation.initialize().unwrap();

        let output = simulation.world().get_block(output_pos);
        assert_eq!(
            simulation
                .rules()
                .registry()
                .state(output)
                .unwrap()
                .property("lit"),
            Some("false")
        );
    }
}

#[test]
fn wooden_pressure_plate_tracks_item_entities_and_releases_after_delay() {
    let mut registry = Java26Registry::new();
    let plate = state(
        &mut registry,
        "minecraft:oak_pressure_plate",
        &[("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, plate).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:item".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::from([
            (
                "item_id".to_owned(),
                serde_json::Value::String("minecraft:stone".to_owned()),
            ),
            ("item_count".to_owned(), serde_json::Value::from(1)),
            ("age".to_owned(), serde_json::Value::from(5_998)),
        ]),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation.step().unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(powered)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit)
            .unwrap()
            .property("lit"),
        Some("true")
    );
    simulation
        .run_until(redstone_core::GameTick(21))
        .unwrap();
    let released = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(released)
            .unwrap()
            .property("powered"),
        Some("false")
    );
    assert_eq!(simulation.world().entities().count(), 0);
}

#[test]
fn detector_rail_responds_only_to_minecarts() {
    let mut registry = Java26Registry::new();
    let rail = state(
        &mut registry,
        "minecraft:detector_rail",
        &[
            ("powered", "false"),
            ("shape", "north_south"),
            ("waterlogged", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, rail).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:hopper_minecart".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::new(),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation.step().unwrap();

    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(powered)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit)
            .unwrap()
            .property("lit"),
        Some("true")
    );
}

#[test]
fn tripwire_powers_a_facing_hook_when_an_entity_intersects() {
    let mut registry = Java26Registry::new();
    let wire = state(
        &mut registry,
        "minecraft:tripwire",
        &[
            ("attached", "true"),
            ("disarmed", "false"),
            ("east", "true"),
            ("north", "false"),
            ("powered", "false"),
            ("south", "false"),
            ("west", "true"),
        ],
    );
    let hook = state(
        &mut registry,
        "minecraft:tripwire_hook",
        &[
            ("attached", "true"),
            ("facing", "west"),
            ("powered", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, wire).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), hook).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), stone).unwrap();
    world.set_block(BlockPos::new(2, 0, 1), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:generic_collision".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::new(),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation.step().unwrap();

    let hook = simulation.world().get_block(BlockPos::new(1, 0, 0));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(hook)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(2, 0, 1));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit)
            .unwrap()
            .property("lit"),
        Some("true")
    );
}

#[test]
fn lectern_emits_a_two_tick_use_pulse() {
    let mut registry = Java26Registry::new();
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[
            ("facing", "north"),
            ("has_book", "true"),
            ("powered", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(
        &mut registry,
        "minecraft:redstone_lamp",
        &[("lit", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lectern).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();

    simulation
        .apply(Action::UseBlock {
            pos: BlockPos::ZERO,
        })
        .unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(powered)
            .unwrap()
            .property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(lit)
            .unwrap()
            .property("lit"),
        Some("true")
    );
    simulation
        .run_until(redstone_core::GameTick(2))
        .unwrap();
    let released = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(released)
            .unwrap()
            .property("powered"),
        Some("false")
    );
}

#[test]
fn piston_rejects_block_entity_container_states_without_runtime_data() {
    let containers: &[(&str, &[(&str, &str)])] = &[
        (
            "minecraft:chest",
            &[
                ("facing", "north"),
                ("type", "single"),
                ("waterlogged", "false"),
            ],
        ),
        (
            "minecraft:hopper",
            &[("enabled", "true"), ("facing", "down")],
        ),
        (
            "minecraft:furnace",
            &[("facing", "north"), ("lit", "false")],
        ),
        (
            "minecraft:barrel",
            &[("facing", "north"), ("open", "false")],
        ),
        (
            "minecraft:dropper",
            &[("facing", "north"), ("triggered", "false")],
        ),
        (
            "minecraft:dispenser",
            &[("facing", "north"), ("triggered", "false")],
        ),
        (
            "minecraft:crafter",
            &[
                ("crafting", "false"),
                ("orientation", "north_up"),
                ("triggered", "false"),
            ],
        ),
    ];

    for &(name, properties) in containers {
        let mut registry = Java26Registry::new();
        let piston = state(
            &mut registry,
            "minecraft:piston",
            &[("extended", "false"), ("facing", "east")],
        );
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let container_state = state(&mut registry, name, properties);
        assert!(registry.state(container_state).unwrap().has_block_entity);

        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, piston).unwrap();
        world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
        world
            .set_block(BlockPos::new(1, 0, 0), container_state)
            .unwrap();
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .unwrap();
        simulation.initialize().unwrap();

        simulation.step().unwrap();

        let piston_state = simulation.world().get_block(BlockPos::ZERO);
        assert_eq!(
            simulation
                .rules()
                .registry()
                .state(piston_state)
                .unwrap()
                .property("extended"),
            Some("false"),
            "{name}"
        );
        assert_eq!(
            simulation.world().get_block(BlockPos::new(1, 0, 0)),
            container_state
        );
        assert_eq!(
            simulation.world().get_block(BlockPos::new(2, 0, 0)),
            simulation.rules().registry().air_state()
        );
    }
}

#[test]
fn piston_moves_slime_branches_without_sticking_to_honey() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let slime = state(&mut registry, "minecraft:slime_block", &[]);
    let honey = state(&mut registry, "minecraft:honey_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), slime).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), honey).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.set_trace_enabled(true);
    simulation.initialize().unwrap();

    simulation.step().unwrap();

    let moving_slime = simulation
        .world()
        .block_entity(BlockPos::new(2, 0, 0))
        .unwrap();
    assert_eq!(moving_slime.kind, "minecraft:moving_piston");
    assert_eq!(moving_slime.fields["moved_state"], slime.0);
    let moving_stone = simulation
        .world()
        .block_entity(BlockPos::new(2, 1, 0))
        .unwrap();
    assert_eq!(moving_stone.fields["moved_state"], stone.0);
    let moving_order = simulation
        .world()
        .block_entities()
        .filter(|(_, data)| data.kind == "minecraft:moving_piston")
        .map(|(pos, data)| (*pos, data.fields["source"].as_bool().unwrap()))
        .collect::<Vec<_>>();
    assert!(moving_order.last().is_some_and(|(_, source)| *source));
    assert!(
        moving_order[..moving_order.len() - 1]
            .iter()
            .all(|(_, source)| !source)
    );
    simulation
        .run_until(redstone_core::GameTick(4))
        .unwrap();

    assert_eq!(simulation.world().get_block(BlockPos::new(2, 0, 0)), slime);
    assert_eq!(simulation.world().get_block(BlockPos::new(2, 1, 0)), stone);
    assert_eq!(simulation.world().get_block(BlockPos::new(1, -1, 0)), honey);
    assert!(
        simulation.trace().events().iter().any(|event| {
            matches!(event.kind, TraceKind::BlockEventExecuted { param_a: 0, .. })
        })
    );
}

#[test]
fn piston_rejects_a_sticky_structure_larger_than_twelve_blocks() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let slime = state(&mut registry, "minecraft:slime_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), slime).unwrap();
    for y in 1..=12 {
        world.set_block(BlockPos::new(1, y, 0), slime).unwrap();
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .unwrap();
    simulation.initialize().unwrap();

    simulation.step().unwrap();

    let piston_state = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation
            .rules()
            .registry()
            .state(piston_state)
            .unwrap()
            .property("extended"),
        Some("false")
    );
    assert_eq!(simulation.world().get_block(BlockPos::new(1, 0, 0)), slime);
}
