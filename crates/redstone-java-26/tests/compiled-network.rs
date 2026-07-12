use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{
    Action, BlockPos, BlockStateId, Direction, ExecutionBackend, ExecutionMode, Probe, Simulation,
    SimulationConfig, SparseWorld, WorldDelta,
};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{
    Java26Registry, Java26Rules, StateResolveError, StateResolver,
};

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

#[test]
fn compiled_subtract_comparator_corner_matches_interpreted_without_events() {
    let lever_pos = BlockPos::new(-31, 1, -32);
    let rear_pos = BlockPos::new(-32, 1, -32);
    let comparator_pos = BlockPos::new(-32, 1, -31);
    let side_pos = BlockPos::new(-31, 1, -31);
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[
            ("face", "floor"),
            ("facing", "north"),
            ("powered", "false"),
        ],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let comparator = state_with(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "subtract"),
            ("powered", "false"),
        ],
    );
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [lever_pos, rear_pos, comparator_pos, side_pos] {
        world.set_block(pos.offset(0, -1, 0), support).unwrap();
    }
    world.set_block(lever_pos, lever).unwrap();
    world.set_block(rear_pos, wire).unwrap();
    world.set_block(comparator_pos, comparator).unwrap();
    world.set_block(side_pos, wire).unwrap();

    let probes = [
        (
            "lever_powered",
            Probe::Property {
                pos: lever_pos,
                property: "powered".to_owned(),
            },
        ),
        (
            "rear_power",
            Probe::Property {
                pos: rear_pos,
                property: "power".to_owned(),
            },
        ),
        (
            "side_power",
            Probe::Property {
                pos: side_pos,
                property: "power".to_owned(),
            },
        ),
        (
            "comparator_powered",
            Probe::Property {
                pos: comparator_pos,
                property: "powered".to_owned(),
            },
        ),
        (
            "comparator_output",
            Probe::Signal {
                pos: comparator_pos,
                direction: Some(Direction::North),
            },
        ),
    ];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_compiled(&compiled);
    assert_world_equal(&interpreted, &compiled);

    for index in 0..14 {
        let actions = if matches!(index, 0 | 7) {
            vec![Action::PullLever { pos: lever_pos }]
        } else {
            Vec::new()
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn auto_raw_paste_falls_back_and_preserves_preset_wire_power() {
    let powered_pos = BlockPos::new(0, 1, 0);
    let idle_pos = BlockPos::new(1, 1, 0);
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let powered_wire = state_with(
        &mut registry,
        "minecraft:redstone_wire",
        &[("power", "15")],
    );
    let idle_wire = state_with(
        &mut registry,
        "minecraft:redstone_wire",
        &[("power", "0")],
    );
    let base = SparseWorld::new(registry.air_state());
    let mut paste = SparseWorld::new(registry.air_state());
    paste
        .set_block(powered_pos.offset(0, -1, 0), support)
        .unwrap();
    paste
        .set_block(idle_pos.offset(0, -1, 0), support)
        .unwrap();
    paste.set_block(powered_pos, powered_wire).unwrap();
    paste.set_block(idle_pos, idle_wire).unwrap();
    let probes = [
        (
            "preset_power",
            Probe::Property {
                pos: powered_pos,
                property: "power".to_owned(),
            },
        ),
        (
            "idle_power",
            Probe::Property {
                pos: idle_pos,
                property: "power".to_owned(),
            },
        ),
    ];
    let common = SimulationConfig {
        record_events: false,
        ..SimulationConfig::default()
    };
    let mut interpreted = Simulation::load(
        Java26Rules::new(registry.clone()),
        base.clone(),
        SimulationConfig {
            execution_mode: ExecutionMode::Interpreted,
            ..common.clone()
        },
    )
    .unwrap();
    let mut compiled = Simulation::load(
        Java26Rules::new(registry),
        base,
        SimulationConfig {
            execution_mode: ExecutionMode::Auto,
            ..common
        },
    )
    .unwrap();
    for (name, probe) in &probes {
        interpreted.add_probe(*name, probe.clone());
        compiled.add_probe(*name, probe.clone());
    }
    for simulation in [&mut interpreted, &mut compiled] {
        simulation
            .paste_world(
                &paste,
                powered_pos.offset(0, -1, 0),
                idle_pos,
                false,
                false,
            )
            .unwrap();
    }
    assert_world_equal(&interpreted, &compiled);
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    let report = compiled.execution_report();
    assert_eq!(report.backend, ExecutionBackend::Interpreted);
    assert!(
        report
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("not a fixed point"))
    );

    for _ in 0..4 {
        let interpreted_delta = interpreted.step().unwrap();
        let compiled_delta = compiled.step().unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
        assert_eq!(compiled.world().get_block(powered_pos), powered_wire);
        assert_eq!(compiled.world().get_block(idle_pos), idle_wire);
    }
}

#[test]
fn compiled_flying_machine_matches_interpreted_without_events() {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/flying-machine.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();
    let button_pos = BlockPos::new(17, 21, 29);
    let button = state_with(
        &mut resolver.0,
        "minecraft:stone_button",
        &[
            ("face", "wall"),
            ("facing", "north"),
            ("powered", "false"),
        ],
    );
    let probes = [
        ("button", Probe::BlockState { pos: button_pos }),
        (
            "machine_origin",
            Probe::BlockState {
                pos: BlockPos::ZERO,
            },
        ),
    ];
    let (mut interpreted, mut compiled) =
        simulation_pair(resolver.0, loaded.world, &probes);
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_compiled(&compiled);

    for index in 0..32 {
        let actions = if index == 19 {
            vec![Action::SetBlock {
                pos: button_pos,
                state: button,
            }]
        } else {
            Vec::new()
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_daylight_source_survives_overlapping_lever_turning_off() {
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let daylight = state_with(
        &mut registry,
        "minecraft:daylight_detector",
        &[("power", "15")],
    );
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[
            ("face", "floor"),
            ("facing", "north"),
            ("powered", "true"),
        ],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let daylight_pos = BlockPos::new(-1, 1, 0);
    let wire_pos = BlockPos::new(0, 1, 0);
    let lever_pos = BlockPos::new(1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [daylight_pos, wire_pos, lever_pos] {
        world.set_block(pos.offset(0, -1, 0), support).unwrap();
    }
    world.set_block(daylight_pos, daylight).unwrap();
    world.set_block(wire_pos, wire).unwrap();
    world.set_block(lever_pos, lever).unwrap();
    let probes = [(
        "wire_power",
        Probe::Property {
            pos: wire_pos,
            property: "power".to_owned(),
        },
    )];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_compiled(&compiled);

    for index in 0..4 {
        let actions = if index == 0 {
            vec![Action::PullLever { pos: lever_pos }]
        } else {
            Vec::new()
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_observer_strong_power_crosses_a_conductor() {
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let observer_off = state_with(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let observer_on = state_with(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "true")],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let observer_pos = BlockPos::new(0, 1, 0);
    let conductor_pos = BlockPos::new(-1, 1, 0);
    let wire_pos = BlockPos::new(-2, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world
        .set_block(observer_pos.offset(0, -1, 0), support)
        .unwrap();
    world.set_block(wire_pos.offset(0, -1, 0), support).unwrap();
    world.set_block(observer_pos, observer_off).unwrap();
    world.set_block(conductor_pos, support).unwrap();
    world.set_block(wire_pos, wire).unwrap();
    let probes = [(
        "wire_power",
        Probe::Property {
            pos: wire_pos,
            property: "power".to_owned(),
        },
    )];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_compiled(&compiled);

    for index in 0..4 {
        let actions = if index == 0 {
            vec![Action::SetBlock {
                pos: observer_pos,
                state: observer_on,
            }]
        } else {
            Vec::new()
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

fn simulation_pair(
    registry: Java26Registry,
    world: SparseWorld,
    probes: &[(&str, Probe)],
) -> (Simulation<Java26Rules>, Simulation<Java26Rules>) {
    let common = SimulationConfig {
        record_events: false,
        ..SimulationConfig::default()
    };
    let mut interpreted = Simulation::load(
        Java26Rules::new(registry.clone()),
        world.clone(),
        SimulationConfig {
            execution_mode: ExecutionMode::Interpreted,
            ..common.clone()
        },
    )
    .unwrap();
    let mut compiled = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..common
        },
    )
    .unwrap();
    for (name, probe) in probes {
        interpreted.add_probe(*name, probe.clone());
        compiled.add_probe(*name, probe.clone());
    }
    (interpreted, compiled)
}

fn assert_tick_equal(
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
    interpreted_delta: WorldDelta,
    compiled_delta: WorldDelta,
) {
    assert_eq!(compiled_delta.tick, interpreted_delta.tick);
    assert_eq!(compiled_delta.probes, interpreted_delta.probes);
    assert_world_equal(interpreted, compiled);
}

fn assert_world_equal(
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
) {
    assert_eq!(compiled.current_tick(), interpreted.current_tick());
    assert_eq!(
        compiled.rules().semantic_cache_fingerprint(),
        interpreted.rules().semantic_cache_fingerprint()
    );
    assert_eq!(compiled.world().air(), interpreted.world().air());
    assert_eq!(
        compiled.pending_scheduled_tick_entries(),
        interpreted.pending_scheduled_tick_entries()
    );
    assert_eq!(
        compiled.world().iter_blocks().collect::<Vec<_>>(),
        interpreted.world().iter_blocks().collect::<Vec<_>>()
    );
    assert_eq!(
        compiled
            .world()
            .block_entities()
            .map(|(pos, data)| (*pos, data.clone()))
            .collect::<Vec<_>>(),
        interpreted
            .world()
            .block_entities()
            .map(|(pos, data)| (*pos, data.clone()))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        compiled
            .world()
            .entities()
            .map(|(id, data)| (*id, data.clone()))
            .collect::<Vec<_>>(),
        interpreted
            .world()
            .entities()
            .map(|(id, data)| (*id, data.clone()))
            .collect::<Vec<_>>()
    );
}

fn assert_compiled(simulation: &Simulation<Java26Rules>) {
    assert_eq!(
        simulation.execution_report().backend,
        ExecutionBackend::Compiled
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
