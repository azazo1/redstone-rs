use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use redstone_core::{
    Action, BlockEntityData, BlockPos, BlockStateId, Direction, ExecutionBackend, ExecutionMode,
    Probe, Simulation, SimulationConfig, SparseWorld, WorldDelta, WorldEvent, WorldPaste,
};
use redstone_io::{
    InitializationMode, Scenario, ScenarioActionKind, StructureLoader, StructureStateResolver,
};
use redstone_java_26::{
    Java26Registry, Java26Rules, StateResolveError, StateResolver,
};

const BUTTON_POS: BlockPos = BlockPos::new(3, 1, 1);

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
fn compiled_wire_and_diode_match_interpreted_each_tick() {
    let mut registry = Java26Registry::new();
    let source = default_state(&mut registry, "minecraft:redstone_block");
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let support = default_state(&mut registry, "minecraft:stone");
    let repeater = state_with(
        &mut registry,
        "minecraft:repeater",
        &[("facing", "west")],
    );
    let lamp = default_state(&mut registry, "minecraft:redstone_lamp");
    let mut world = SparseWorld::new(registry.air_state());
    for x in 0..=3 {
        world
            .set_block(BlockPos::new(x, 0, 0), support)
            .unwrap();
    }
    world.set_block(BlockPos::new(0, 1, 0), wire).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), wire).unwrap();
    world
        .set_block(BlockPos::new(2, 1, 0), repeater)
        .unwrap();
    world.set_block(BlockPos::new(3, 1, 0), lamp).unwrap();

    let probes = [
        (
            "wire_power",
            Probe::Property {
                pos: BlockPos::new(1, 1, 0),
                property: "power".to_owned(),
            },
        ),
        (
            "lamp_lit",
            Probe::Property {
                pos: BlockPos::new(3, 1, 0),
                property: "lit".to_owned(),
            },
        ),
    ];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    assert_simulations_equal(&interpreted, &compiled);

    for index in 0..12 {
        let actions = match index {
            0 => vec![Action::SetBlock {
                pos: BlockPos::new(-1, 1, 0),
                state: source,
            }],
            6 => vec![Action::BreakBlock {
                pos: BlockPos::new(-1, 1, 0),
            }],
            _ => Vec::new(),
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

    let report = compiled.execution_report();
    assert_eq!(report.backend, ExecutionBackend::Compiled);
    assert!(report.compiled_updates > 0);
    assert!(report.recompiled_nodes > 0);
}

#[test]
fn compiled_source_state_paste_matches_interpreted_each_tick() {
    let mut registry = Java26Registry::new();
    let lever_off = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let lever_on = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "true")],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let support = default_state(&mut registry, "minecraft:stone");
    let lever_pos = BlockPos::new(-1, 1, 0);
    let wire_pos = BlockPos::new(0, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world
        .set_block(lever_pos.relative(Direction::Down), support)
        .unwrap();
    world
        .set_block(wire_pos.relative(Direction::Down), support)
        .unwrap();
    world.set_block(lever_pos, lever_off).unwrap();
    world.set_block(wire_pos, wire).unwrap();

    let probes = [("wire_power", Probe::Property {
        pos: wire_pos,
        property: "power".to_owned(),
    })];
    let (mut interpreted, mut compiled) = simulation_pair(registry.clone(), world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_eq!(
        compiled.execution_report().backend,
        ExecutionBackend::Compiled
    );

    let mut powered = SparseWorld::new(registry.air_state());
    powered.set_block(lever_pos, lever_on).unwrap();
    let mut unpowered = SparseWorld::new(registry.air_state());
    unpowered.set_block(lever_pos, lever_off).unwrap();
    for paste in [Some(&powered), None, Some(&unpowered), None] {
        let interpreted_delta = if let Some(source) = paste {
            interpreted
                .step_with_pastes_and_actions(
                    &[WorldPaste {
                        source,
                        region_min: lever_pos,
                        region_max: lever_pos,
                        ignore_air: false,
                        paste_entities: false,
                        update: true,
                    }],
                    &[],
                )
                .unwrap()
        } else {
            interpreted.step().unwrap()
        };
        let compiled_delta = if let Some(source) = paste {
            compiled
                .step_with_pastes_and_actions(
                    &[WorldPaste {
                        source,
                        region_min: lever_pos,
                        region_max: lever_pos,
                        ignore_air: false,
                        paste_entities: false,
                        update: true,
                    }],
                    &[],
                )
                .unwrap()
        } else {
            compiled.step().unwrap()
        };
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_wire_shape_paste_activates_new_consumer_edges() {
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let lamp = default_state(&mut registry, "minecraft:redstone_lamp");
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let disconnected_wire = state_with(
        &mut registry,
        "minecraft:redstone_wire",
        &[
            ("north", "none"),
            ("east", "side"),
            ("south", "none"),
            ("west", "none"),
        ],
    );
    let connected_wire = state_with(
        &mut registry,
        "minecraft:redstone_wire",
        &[
            ("north", "none"),
            ("east", "side"),
            ("south", "none"),
            ("west", "side"),
        ],
    );
    let lamp_pos = BlockPos::new(-1, 1, 0);
    let wire_pos = BlockPos::new(0, 1, 0);
    let lever_pos = BlockPos::new(1, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [lamp_pos, wire_pos, lever_pos] {
        world.set_block(pos.relative(Direction::Down), support).unwrap();
    }
    world.set_block(lamp_pos, lamp).unwrap();
    world.set_block(wire_pos, disconnected_wire).unwrap();
    world.set_block(lever_pos, lever).unwrap();
    let probes = [("lamp_lit", Probe::Property {
        pos: lamp_pos,
        property: "lit".to_owned(),
    })];
    let (mut interpreted, mut compiled) = simulation_pair(registry.clone(), world, &probes);
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();

    let mut paste = SparseWorld::new(registry.air_state());
    paste.set_block(wire_pos, connected_wire).unwrap();
    let world_paste = WorldPaste {
        source: &paste,
        region_min: wire_pos,
        region_max: wire_pos,
        ignore_air: false,
        paste_entities: false,
        update: false,
    };
    let actions = [Action::PullLever { pos: lever_pos }];
    let interpreted_delta = interpreted
        .step_with_pastes_and_actions(&[world_paste], &actions)
        .unwrap();
    let compiled_delta = compiled
        .step_with_pastes_and_actions(&[world_paste], &actions)
        .unwrap();
    assert_tick_equal(
        &interpreted,
        &compiled,
        interpreted_delta,
        compiled_delta,
    );
    for _ in 0..3 {
        let interpreted_delta = interpreted.step().unwrap();
        let compiled_delta = compiled.step().unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_comparator_block_entity_paste_matches_interpreted_each_tick() {
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let comparator = state_with(
        &mut registry,
        "minecraft:comparator",
        &[("facing", "north"), ("powered", "false")],
    );
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let comparator_pos = BlockPos::new(0, 1, 0);
    let wire_positions = [BlockPos::new(0, 1, 1), BlockPos::new(0, 1, 2)];
    let lever_pos = BlockPos::new(1, 1, 2);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [comparator_pos, wire_positions[0], wire_positions[1], lever_pos] {
        world.set_block(pos.relative(Direction::Down), support).unwrap();
    }
    world.set_block(comparator_pos, comparator).unwrap();
    world.set_block(wire_positions[0], wire).unwrap();
    world.set_block(wire_positions[1], wire).unwrap();
    world.set_block(lever_pos, lever).unwrap();
    world.set_block_entity(comparator_pos, comparator_data(0));
    let probes = [("wire_power", Probe::Property {
        pos: wire_positions[0],
        property: "power".to_owned(),
    })];
    let (mut interpreted, mut compiled) = simulation_pair(registry.clone(), world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();

    let mut paste = SparseWorld::new(registry.air_state());
    paste.set_block(comparator_pos, comparator).unwrap();
    paste.set_block_entity(comparator_pos, comparator_data(7));
    let world_paste = WorldPaste {
        source: &paste,
        region_min: comparator_pos,
        region_max: comparator_pos,
        ignore_air: false,
        paste_entities: false,
        update: false,
    };
    let interpreted_delta = interpreted
        .step_with_pastes_and_actions(&[world_paste], &[])
        .unwrap();
    let compiled_delta = compiled
        .step_with_pastes_and_actions(&[world_paste], &[])
        .unwrap();
    assert_tick_equal(
        &interpreted,
        &compiled,
        interpreted_delta,
        compiled_delta,
    );

    for _ in 0..2 {
        let actions = [Action::PullLever { pos: lever_pos }];
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

fn comparator_data(output: i64) -> BlockEntityData {
    BlockEntityData {
        kind: "minecraft:comparator".to_owned(),
        fields: BTreeMap::from([(
            "comparator_output".to_owned(),
            serde_json::Value::from(output),
        )]),
    }
}

#[test]
fn compiled_fast_path_matches_interpreted_world_without_event_recording() {
    let mut registry = Java26Registry::new();
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let support = default_state(&mut registry, "minecraft:stone");
    let repeater = state_with(
        &mut registry,
        "minecraft:repeater",
        &[("facing", "west")],
    );
    let lamp = default_state(&mut registry, "minecraft:redstone_lamp");
    let lever_pos = BlockPos::new(-1, 1, 0);
    let wire_positions = [
        BlockPos::new(0, 1, 0),
        BlockPos::new(1, 1, 0),
        BlockPos::new(1, 1, -1),
        BlockPos::new(1, 1, 1),
    ];
    let repeater_pos = BlockPos::new(2, 1, 0);
    let lamp_pos = BlockPos::new(3, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for x in -1..=3 {
        for z in -1..=1 {
            world
                .set_block(BlockPos::new(x, 0, z), support)
                .unwrap();
        }
    }
    world.set_block(lever_pos, lever).unwrap();
    for pos in wire_positions {
        world.set_block(pos, wire).unwrap();
    }
    world.set_block(repeater_pos, repeater).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();

    let probes = [
        (
            "branch_power",
            Probe::Property {
                pos: wire_positions[3],
                property: "power".to_owned(),
            },
        ),
        (
            "lamp_lit",
            Probe::Property {
                pos: lamp_pos,
                property: "lit".to_owned(),
            },
        ),
    ];
    let config = SimulationConfig {
        record_events: false,
        ..SimulationConfig::default()
    };
    let mut interpreted = Simulation::load(
        Java26Rules::new(registry.clone()),
        world.clone(),
        SimulationConfig {
            execution_mode: ExecutionMode::Interpreted,
            ..config.clone()
        },
    )
    .unwrap();
    let mut compiled = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..config
        },
    )
    .unwrap();
    for (name, probe) in probes {
        interpreted.add_probe(name, probe.clone());
        compiled.add_probe(name, probe);
    }
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    assert_simulations_equal(&interpreted, &compiled);

    for index in 0..16 {
        let actions = if matches!(index, 0 | 8) {
            vec![Action::PullLever { pos: lever_pos }]
        } else {
            Vec::new()
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        assert!(interpreted_delta.events.is_empty());
        assert!(compiled_delta.events.is_empty());
        assert_eq!(compiled_delta.probes, interpreted_delta.probes);
        assert_simulations_equal(&interpreted, &compiled);
    }
}

#[test]
fn compiled_triple_piston_matches_interpreted_each_tick() {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/tripple-piston-extender.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();
    let probes = [
        (
            "button_powered",
            Probe::Property {
                pos: BUTTON_POS,
                property: "powered".to_owned(),
            },
        ),
        (
            "moving_payload",
            Probe::BlockState {
                pos: BlockPos::new(8, 0, 5),
            },
        ),
    ];
    let (mut interpreted, mut compiled) =
        simulation_pair(resolver.0, loaded.world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    assert_simulations_equal(&interpreted, &compiled);

    for index in 0..104 {
        let actions = if matches!(index, 0 | 52) {
            vec![Action::PressButton { pos: BUTTON_POS }]
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

    let report = compiled.execution_report();
    assert_eq!(report.backend, ExecutionBackend::Compiled);
    assert!(report.recompiled_nodes > 0);
}

#[test]
fn compiled_t_shaped_wire_preserves_complete_delta_order_each_tick() {
    let mut registry = Java26Registry::new();
    let source = default_state(&mut registry, "minecraft:redstone_block");
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let support = default_state(&mut registry, "minecraft:stone");
    let center = BlockPos::new(0, 1, 0);
    let branches = [
        center,
        BlockPos::new(1, 1, 0),
        BlockPos::new(0, 1, -1),
        BlockPos::new(0, 1, 1),
    ];
    let mut world = SparseWorld::new(registry.air_state());
    for pos in branches {
        world
            .set_block(BlockPos::new(pos.x, 0, pos.z), support)
            .unwrap();
        world.set_block(pos, wire).unwrap();
    }
    let probes = branches.map(|pos| {
        (
            match (pos.x, pos.z) {
                (0, 0) => "center_power",
                (1, 0) => "east_power",
                (0, -1) => "north_power",
                (0, 1) => "south_power",
                _ => unreachable!(),
            },
            Probe::Property {
                pos,
                property: "power".to_owned(),
            },
        )
    });
    let source_pos = BlockPos::new(-1, 1, 0);
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    assert_simulations_equal(&interpreted, &compiled);

    for index in 0..6 {
        let actions = match index {
            0 => vec![Action::SetBlock {
                pos: source_pos,
                state: source,
            }],
            3 => vec![Action::BreakBlock { pos: source_pos }],
            _ => Vec::new(),
        };
        let interpreted_delta = interpreted.step_with_actions(&actions).unwrap();
        let compiled_delta = compiled.step_with_actions(&actions).unwrap();
        if matches!(index, 0 | 3) {
            assert!(!interpreted_delta.events.is_empty());
        }
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_wire_does_not_treat_weak_redstone_block_power_as_conductor_input() {
    let mut registry = Java26Registry::new();
    let source = default_state(&mut registry, "minecraft:redstone_block");
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let conductor = default_state(&mut registry, "minecraft:stone");
    let wire_pos = BlockPos::new(0, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(wire_pos, wire).unwrap();
    world
        .set_block(wire_pos.relative(redstone_core::Direction::Down), conductor)
        .unwrap();
    world
        .set_block(BlockPos::new(1, 0, 0), source)
        .unwrap();
    let probes = [(
        "wire_power",
        Probe::Property {
            pos: wire_pos,
            property: "power".to_owned(),
        },
    )];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);

    for _ in 0..4 {
        let interpreted_delta = interpreted.step().unwrap();
        let compiled_delta = compiled.step().unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
}

#[test]
fn compiled_flying_machine_matches_interpreted_during_topology_changes() {
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
                pos: BlockPos::new(0, 0, 0),
            },
        ),
    ];
    let (mut interpreted, mut compiled) =
        simulation_pair(resolver.0, loaded.world, &probes);

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

    let report = compiled.execution_report();
    assert_eq!(report.backend, ExecutionBackend::Compiled);
    assert!(report.recompiled_nodes > 0);
}

#[test]
fn compiled_moved_observer_matches_interpreted_each_tick() {
    let mut registry = Java26Registry::new();
    let piston = state_with(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = default_state(&mut registry, "minecraft:redstone_block");
    let observer = state_with(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let moved_pos = BlockPos::new(2, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), observer).unwrap();
    let probes = [("moved_observer", Probe::BlockState { pos: moved_pos })];
    let (mut interpreted, mut compiled) = simulation_pair(registry, world, &probes);
    interpreted.initialize().unwrap();
    compiled.initialize().unwrap();
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();

    for _ in 0..5 {
        let interpreted_delta = interpreted.step().unwrap();
        let compiled_delta = compiled.step().unwrap();
        assert_tick_equal(
            &interpreted,
            &compiled,
            interpreted_delta,
            compiled_delta,
        );
    }
    assert_eq!(
        compiled.execution_report().backend,
        ExecutionBackend::Compiled
    );
}

#[test]
fn compiled_push_destroy_components_match_interpreted_each_tick() {
    run_workspace_scenario_differential("piston-push-destroy-tests.toml", true, None);
}

#[test]
fn compiled_push_destroy_attachments_match_interpreted_each_tick() {
    run_workspace_scenario_differential("piston-push-destroy-tests-2.toml", true, None);
}

#[test]
#[ignore = "动态场景逐 tick 差分耗时较长"]
fn compiled_3x3_piston_gate_matches_interpreted_each_tick() {
    run_workspace_scenario_differential("piston-gate-3x3.toml", true, None);
}

#[test]
#[ignore = "动态场景逐 tick 差分耗时较长"]
fn compiled_flying_roof_matches_interpreted_each_tick() {
    run_workspace_scenario_differential("flying-roof.toml", true, None);
}

#[test]
#[ignore = "完整 flying machine 逐 tick 差分耗时较长"]
fn compiled_full_flying_machine_matches_interpreted_each_tick() {
    run_workspace_scenario_differential("flying-machine.toml", true, None);
}

#[test]
#[ignore = "Frostbyte 逐 tick 编译诊断耗时较长"]
fn frostbyte_hello_world_reports_first_compiled_tick_difference() {
    run_workspace_scenario_differential(
        "frostbyte-cpu-16bit-hello-world.toml",
        true,
        None,
    );
}

#[test]
#[ignore = "Frostbyte 无事件高速路径逐 tick 诊断耗时较长"]
fn frostbyte_hello_world_reports_first_fast_tick_difference() {
    run_workspace_scenario_differential(
        "frostbyte-cpu-16bit-hello-world.toml",
        false,
        Some(140),
    );
}

#[test]
#[ignore = "Frostbyte line-drawing 逐 tick 编译诊断耗时较长"]
fn frostbyte_line_drawing_reports_first_compiled_tick_difference() {
    run_workspace_scenario_differential(
        "frostbyte-cpu-16bit-line-drawing.toml",
        true,
        None,
    );
}

#[test]
fn compiled_static_circuit_is_deterministic() {
    let mut registry = Java26Registry::new();
    let support = default_state(&mut registry, "minecraft:stone");
    let lever = state_with(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let wire = default_state(&mut registry, "minecraft:redstone_wire");
    let lamp = default_state(&mut registry, "minecraft:redstone_lamp");
    let lever_pos = BlockPos::new(0, 1, 0);
    let wire_pos = BlockPos::new(1, 1, 0);
    let lamp_pos = BlockPos::new(2, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for pos in [lever_pos, wire_pos, lamp_pos] {
        world.set_block(pos.relative(Direction::Down), support).unwrap();
    }
    world.set_block(lever_pos, lever).unwrap();
    world.set_block(wire_pos, wire).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let probes = [("lamp", Probe::BlockState { pos: lamp_pos })];
    let mut left = compiled_simulation(registry.clone(), world.clone(), &probes);
    let mut right = compiled_simulation(registry, world, &probes);

    assert_deterministic_compiled_runs(&mut left, &mut right, 8, |tick| {
        if matches!(tick, 0 | 4) {
            vec![Action::PullLever { pos: lever_pos }]
        } else {
            Vec::new()
        }
    });
    assert_eq!(left.execution_report().dense_full_rebuilds, 0);
}

#[test]
fn compiled_piston_topology_is_deterministic() {
    let mut registry = Java26Registry::new();
    let piston = state_with(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = default_state(&mut registry, "minecraft:redstone_block");
    let observer = state_with(
        &mut registry,
        "minecraft:observer",
        &[("facing", "east"), ("powered", "false")],
    );
    let moved_pos = BlockPos::new(2, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), observer).unwrap();
    let probes = [("moved_observer", Probe::BlockState { pos: moved_pos })];
    let mut left = compiled_simulation(registry.clone(), world.clone(), &probes);
    let mut right = compiled_simulation(registry, world, &probes);

    assert_deterministic_compiled_runs(&mut left, &mut right, 6, |_| Vec::new());
    assert!(left.execution_report().dense_full_rebuilds > 0);
}

fn run_workspace_scenario_differential(
    scenario_name: &str,
    record_events: bool,
    tick_limit: Option<u64>,
) {
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenarios")
        .join(scenario_name);
    let scenario = Scenario::load(&scenario_path).unwrap();
    let mut resolver = RegistryResolver(Java26Registry::new());
    let base = StructureLoader::load_with_options(
        &scenario.source.path,
        scenario.source.load_options(),
        &mut resolver,
    )
    .unwrap();
    let loaded_pastes = scenario
        .source
        .pastes
        .iter()
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
    let mut resolved_actions = scenario
        .actions
        .iter()
        .map(|action| {
            (
                action.tick,
                resolve_diagnostic_action(&mut resolver.0, &action.action),
            )
        })
        .collect::<Vec<_>>();
    resolved_actions.sort_by_key(|(tick, _)| *tick);
    let common_config = SimulationConfig {
        mode: scenario.mode,
        seed: scenario.seed,
        environment: scenario.environment.into(),
        strict: scenario.strict,
        record_events,
        ..SimulationConfig::default()
    };
    let mut interpreted = Simulation::load(
        Java26Rules::new(resolver.0.clone()),
        base.world.clone(),
        SimulationConfig {
            execution_mode: ExecutionMode::Interpreted,
            ..common_config.clone()
        },
    )
    .unwrap();
    let mut compiled = Simulation::load(
        Java26Rules::new(resolver.0),
        base.world,
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..common_config
        },
    )
    .unwrap();
    for probe in &scenario.probes {
        interpreted.add_probe(&probe.name, probe.probe.clone());
        compiled.add_probe(&probe.name, probe.probe.clone());
    }
    let mut monitoring_enabled = scenario.monitor.skip_ticks == 0;
    interpreted.set_monitoring_enabled(monitoring_enabled);
    compiled.set_monitoring_enabled(monitoring_enabled);

    if scenario.source.initialization == InitializationMode::Notify {
        interpreted.initialize().unwrap();
        compiled.initialize().unwrap();
    }
    for (paste, structure) in loaded_pastes
        .iter()
        .filter(|(paste, _)| paste.tick.is_none())
    {
        for simulation in [&mut interpreted, &mut compiled] {
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
    }
    assert_simulations_equal(&interpreted, &compiled);
    interpreted.prepare_execution().unwrap();
    compiled.prepare_execution().unwrap();
    assert_eq!(
        compiled.execution_report().backend,
        ExecutionBackend::Compiled,
        "场景必须使用强制 compiled 后端: {}",
        scenario_path.display()
    );
    eprintln!(
        "compiled report after prepare for {}: {:?}",
        scenario_path.display(),
        compiled.execution_report()
    );

    let target_tick = tick_limit.unwrap_or(scenario.max_ticks).min(scenario.max_ticks);
    while interpreted.current_tick().0 < target_tick {
        let next_tick = interpreted.current_tick().0 + 1;
        if !monitoring_enabled && next_tick > scenario.monitor.skip_ticks {
            interpreted.set_monitoring_enabled(true);
            compiled.set_monitoring_enabled(true);
            monitoring_enabled = true;
        }
        let pastes = loaded_pastes
            .iter()
            .filter(|(paste, _)| paste.tick.is_some_and(|tick| tick.0 == next_tick))
            .map(|(paste, structure)| WorldPaste {
                source: &structure.world,
                region_min: structure.region_min,
                region_max: structure.region_max,
                ignore_air: paste.ignore_air,
                paste_entities: paste.paste_entities,
                update: paste.update,
            })
            .collect::<Vec<_>>();
        let actions = resolved_actions
            .iter()
            .filter(|(tick, _)| tick.0 == next_tick)
            .map(|(_, action)| action.clone())
            .collect::<Vec<_>>();
        let interpreted_delta = interpreted
            .step_with_pastes_and_actions(&pastes, &actions)
            .unwrap();
        let compiled_delta = compiled
            .step_with_pastes_and_actions(&pastes, &actions)
            .unwrap();
        let interpreted_pending = interpreted.pending_scheduled_tick_entries();
        let compiled_pending = compiled.pending_scheduled_tick_entries();
        if interpreted_delta != compiled_delta
            || interpreted_pending != compiled_pending
            || !simulations_are_equal(&interpreted, &compiled)
        {
            panic!(
                "{}",
                scenario_difference_report(
                    &scenario_path,
                    next_tick,
                    &actions,
                    &interpreted,
                    &compiled,
                    &interpreted_delta,
                    &compiled_delta,
                )
            );
        }
        if next_tick.is_multiple_of(100) {
            eprintln!(
                "diagnosed {} tick {next_tick}, pending_ticks={}, compiled_updates={}",
                scenario_path.display(),
                interpreted_pending.len(),
                compiled.execution_report().compiled_updates
            );
        }
    }
}

fn resolve_diagnostic_action(
    registry: &mut Java26Registry,
    action: &ScenarioActionKind,
) -> Action {
    match action {
        ScenarioActionKind::SetBlock {
            pos,
            name,
            properties,
        } => Action::SetBlock {
            pos: *pos,
            state: registry.resolve_state(name, properties).unwrap(),
        },
        ScenarioActionKind::BreakBlock { pos } => Action::BreakBlock { pos: *pos },
        ScenarioActionKind::UseBlock { pos } => Action::UseBlock { pos: *pos },
        ScenarioActionKind::PressButton { pos } => Action::PressButton { pos: *pos },
        ScenarioActionKind::PullLever { pos } => Action::PullLever { pos: *pos },
        ScenarioActionKind::SetBlockEntity { pos, data } => Action::SetBlockEntity {
            pos: *pos,
            data: data.clone(),
        },
        ScenarioActionKind::SpawnEntity {
            id,
            kind,
            position,
            fields,
        } => Action::SpawnEntity {
            id: *id,
            data: ScenarioActionKind::entity_data(kind.clone(), *position, fields.clone()),
        },
        ScenarioActionKind::MoveEntity { id, position } => Action::MoveEntity {
            id: *id,
            position: *position,
        },
        ScenarioActionKind::RemoveEntity { id } => Action::RemoveEntity { id: *id },
        ScenarioActionKind::SetEntityField { id, field, value } => Action::SetEntityField {
            id: *id,
            field: field.clone(),
            value: value.clone(),
        },
        ScenarioActionKind::HitTarget {
            pos,
            face,
            location,
            arrow,
        } => Action::HitTarget {
            pos: *pos,
            face: *face,
            location: *location,
            arrow: *arrow,
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn scenario_difference_report(
    scenario_path: &std::path::Path,
    tick: u64,
    actions: &[Action],
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
    interpreted_delta: &WorldDelta,
    compiled_delta: &WorldDelta,
) -> String {
    let first_difference = interpreted_delta
        .events
        .iter()
        .zip(&compiled_delta.events)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| {
            interpreted_delta
                .events
                .len()
                .min(compiled_delta.events.len())
        });
    let window_start = first_difference.saturating_sub(4);
    let interpreted_end = (first_difference + 5).min(interpreted_delta.events.len());
    let compiled_end = (first_difference + 5).min(compiled_delta.events.len());
    let interpreted_events = &interpreted_delta.events[window_start.min(interpreted_end)..interpreted_end];
    let compiled_events = &compiled_delta.events[window_start.min(compiled_end)..compiled_end];
    let mut positions = BTreeSet::new();
    for event in interpreted_events.iter().chain(compiled_events) {
        positions.insert(world_event_pos(event));
    }
    for action in actions {
        if let Some(pos) = action_pos(action) {
            positions.insert(pos);
        }
    }
    for tick in interpreted
        .pending_scheduled_tick_entries()
        .into_iter()
        .chain(compiled.pending_scheduled_tick_entries())
    {
        positions.insert(tick.pos);
    }
    if positions.is_empty() {
        for (pos, state) in interpreted.world().iter_blocks() {
            if compiled.world().get_block(pos) != state {
                positions.insert(pos);
                if positions.len() >= 16 {
                    break;
                }
            }
        }
    }
    let centers = positions.iter().copied().collect::<Vec<_>>();
    for pos in centers {
        for direction in Direction::UPDATE_ORDER {
            positions.insert(pos.relative(direction));
        }
    }
    let interpreted_states = positions
        .iter()
        .map(|pos| describe_position(interpreted, *pos))
        .collect::<Vec<_>>()
        .join("\n");
    let compiled_states = positions
        .iter()
        .map(|pos| describe_position(compiled, *pos))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "场景 {} 首次差异位于 tick {tick}\n\
         actions: {actions:?}\n\
         first_event_difference: {first_difference}\n\
         interpreted_event_count: {}\n\
         compiled_event_count: {}\n\
         interpreted_pending_ticks: {}\n\
         compiled_pending_ticks: {}\n\
         interpreted_pending_entries: {:?}\n\
         compiled_pending_entries: {:?}\n\
         interpreted_cache_fingerprint: {:?}\n\
         compiled_cache_fingerprint: {:?}\n\
         interpreted_comparator_outputs: {:?}\n\
         compiled_comparator_outputs: {:?}\n\
         interpreted_events[{window_start}..{interpreted_end}]: {interpreted_events:#?}\n\
         compiled_events[{window_start}..{compiled_end}]: {compiled_events:#?}\n\
         interpreted_related_states:\n{interpreted_states}\n\
         compiled_related_states:\n{compiled_states}\n\
         compiled_report: {:#?}",
        scenario_path.display(),
        interpreted_delta.events.len(),
        compiled_delta.events.len(),
        interpreted.pending_scheduled_ticks(),
        compiled.pending_scheduled_ticks(),
        interpreted.pending_scheduled_tick_entries(),
        compiled.pending_scheduled_tick_entries(),
        interpreted.rules().semantic_cache_fingerprint(),
        compiled.rules().semantic_cache_fingerprint(),
        interpreted.rules().comparator_output_cache(),
        compiled.rules().comparator_output_cache(),
        compiled.execution_report(),
    )
}

fn world_event_pos(event: &WorldEvent) -> BlockPos {
    match event {
        WorldEvent::Block { change } => change.pos,
        WorldEvent::BlockEntity { change } => change.pos(),
        WorldEvent::BlockEvent { event, .. } => event.pos,
    }
}

fn action_pos(action: &Action) -> Option<BlockPos> {
    match action {
        Action::SetBlock { pos, .. }
        | Action::BreakBlock { pos }
        | Action::UseBlock { pos }
        | Action::PressButton { pos }
        | Action::PullLever { pos }
        | Action::SetBlockEntity { pos, .. }
        | Action::HitTarget { pos, .. } => Some(*pos),
        Action::SpawnEntity { .. }
        | Action::MoveEntity { .. }
        | Action::RemoveEntity { .. }
        | Action::SetEntityField { .. } => None,
    }
}

fn describe_position(simulation: &Simulation<Java26Rules>, pos: BlockPos) -> String {
    let state_id = simulation.world().get_block(pos);
    let state = simulation
        .rules()
        .registry()
        .state(state_id)
        .expect("world state must exist in registry");
    format!("{pos:?}: {state_id:?} {} {:?}", state.name, state.properties)
}

fn simulation_pair(
    registry: Java26Registry,
    world: SparseWorld,
    probes: &[(&str, Probe)],
) -> (Simulation<Java26Rules>, Simulation<Java26Rules>) {
    let mut interpreted = Simulation::load(
        Java26Rules::new(registry.clone()),
        world.clone(),
        SimulationConfig {
            execution_mode: ExecutionMode::Interpreted,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    let mut compiled = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    for (name, probe) in probes {
        interpreted.add_probe(*name, probe.clone());
        compiled.add_probe(*name, probe.clone());
    }
    (interpreted, compiled)
}

fn compiled_simulation(
    registry: Java26Registry,
    world: SparseWorld,
    probes: &[(&str, Probe)],
) -> Simulation<Java26Rules> {
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    for (name, probe) in probes {
        simulation.add_probe(*name, probe.clone());
    }
    simulation
}

fn assert_deterministic_compiled_runs(
    left: &mut Simulation<Java26Rules>,
    right: &mut Simulation<Java26Rules>,
    ticks: usize,
    mut actions: impl FnMut(usize) -> Vec<Action>,
) {
    left.initialize().unwrap();
    right.initialize().unwrap();
    left.prepare_execution().unwrap();
    right.prepare_execution().unwrap();
    for tick in 0..ticks {
        let actions = actions(tick);
        let left_delta = left.step_with_actions(&actions).unwrap();
        let right_delta = right.step_with_actions(&actions).unwrap();
        assert_eq!(left_delta, right_delta);
        assert_simulations_equal(left, right);
    }
    let left_report = left.execution_report();
    let right_report = right.execution_report();
    assert_eq!(
        (
            left_report.backend,
            left_report.node_count,
            left_report.edge_count,
            left_report.compiled_updates,
            left_report.interpreted_updates,
            left_report.partial_recompilations,
            left_report.full_recompilations,
            left_report.dense_full_rebuilds,
            left_report.recompiled_nodes,
        ),
        (
            right_report.backend,
            right_report.node_count,
            right_report.edge_count,
            right_report.compiled_updates,
            right_report.interpreted_updates,
            right_report.partial_recompilations,
            right_report.full_recompilations,
            right_report.dense_full_rebuilds,
            right_report.recompiled_nodes,
        )
    );
}

fn simulations_are_equal(
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
) -> bool {
    interpreted.current_tick() == compiled.current_tick()
        && interpreted.environment() == compiled.environment()
        && interpreted.rules().semantic_cache_fingerprint()
            == compiled.rules().semantic_cache_fingerprint()
        && interpreted.world().air() == compiled.world().air()
        && interpreted
            .world()
            .iter_blocks()
            .eq(compiled.world().iter_blocks())
        && interpreted
            .world()
            .block_entities()
            .eq(compiled.world().block_entities())
        && interpreted
            .world()
            .entities()
            .eq(compiled.world().entities())
}

fn assert_tick_equal(
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
    interpreted_delta: WorldDelta,
    compiled_delta: WorldDelta,
) {
    assert_simulations_equal(interpreted, compiled);
    assert_eq!(compiled_delta, interpreted_delta);
}

fn assert_simulations_equal(
    interpreted: &Simulation<Java26Rules>,
    compiled: &Simulation<Java26Rules>,
) {
    assert_eq!(compiled.current_tick(), interpreted.current_tick());
    assert_eq!(compiled.environment(), interpreted.environment());
    assert_eq!(
        compiled.rules().semantic_cache_fingerprint(),
        interpreted.rules().semantic_cache_fingerprint()
    );
    assert_eq!(compiled.world().air(), interpreted.world().air());
    assert_eq!(
        compiled.pending_scheduled_tick_entries(),
        interpreted.pending_scheduled_tick_entries()
    );
    let interpreted_blocks = interpreted.world().iter_blocks().collect::<BTreeMap<_, _>>();
    let compiled_blocks = compiled.world().iter_blocks().collect::<BTreeMap<_, _>>();
    assert_eq!(compiled_blocks, interpreted_blocks);

    let interpreted_block_entities = interpreted
        .world()
        .block_entities()
        .map(|(pos, data)| (*pos, data.clone()))
        .collect::<BTreeMap<_, _>>();
    let compiled_block_entities = compiled
        .world()
        .block_entities()
        .map(|(pos, data)| (*pos, data.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(compiled_block_entities, interpreted_block_entities);

    let interpreted_entities = interpreted
        .world()
        .entities()
        .map(|(id, data)| (*id, data.clone()))
        .collect::<BTreeMap<_, _>>();
    let compiled_entities = compiled
        .world()
        .entities()
        .map(|(id, data)| (*id, data.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(compiled_entities, interpreted_entities);
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
