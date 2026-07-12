use std::collections::BTreeMap;

use redstone_core::{
    BlockPos, BlockStateId, ExecutionBackend, ExecutionMode, Simulation, SimulationConfig,
    SimulationError, SparseWorld,
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
fn auto_mode_falls_back_to_interpreted_when_trace_is_enabled() {
    let registry = Java26Registry::new();
    let world = SparseWorld::new(registry.air_state());
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default()).unwrap();

    assert_eq!(
        simulation.execution_report().backend,
        ExecutionBackend::Compiled
    );
    simulation.set_trace_enabled(true).unwrap();

    let report = simulation.execution_report();
    assert_eq!(report.requested_mode, ExecutionMode::Auto);
    assert_eq!(report.backend, ExecutionBackend::Interpreted);
    assert!(report.fallback_reason.is_some());
}

#[test]
fn compiled_mode_rejects_trace_and_restores_the_previous_configuration() {
    let mut registry = Java26Registry::new();
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("delay", "1"),
            ("facing", "north"),
            ("locked", "false"),
            ("powered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, repeater).unwrap();
    world
        .set_block(BlockPos::new(0, 0, -1), source)
        .unwrap();
    let rules = Java26Rules::new(registry);
    let config = SimulationConfig {
        execution_mode: ExecutionMode::Compiled,
        ..SimulationConfig::default()
    };
    let mut simulation = Simulation::load(rules, world, config).unwrap();

    assert!(matches!(
        simulation.set_trace_enabled(true),
        Err(SimulationError::Rules(_))
    ));
    let report = simulation.execution_report();
    assert_eq!(report.requested_mode, ExecutionMode::Compiled);
    assert_eq!(report.backend, ExecutionBackend::Compiled);
    assert!(report.fallback_reason.is_none());

    simulation.initialize().unwrap();
    assert!(simulation.trace().events().is_empty());
}
