use std::collections::BTreeMap;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use redstone_core::{
    Action, BlockChange, BlockEntityChange, BlockEntityData, BlockEvent, BlockKindId, BlockPos,
    BlockRules, BlockStateId, DeferredBlockChange, DeferredBlockEntityUpdate, EventContext,
    ExecutionBackend, ExecutionConfig, ExecutionMode, ExecutionReport, NeighborUpdate, Probe,
    ProbeValue, RedstoneMode, RulesError, ScheduledTick, Simulation, SimulationConfig,
    SimulationEnvironment, SimulationError, SparseWorld, TickPriority, TraceKind, WorldEvent,
};
use tracing::{Event, Level, Subscriber, field::Visit};
use tracing_subscriber::{layer::Context, prelude::*};

const AIR: BlockStateId = BlockStateId(0);
const BLOCK: BlockStateId = BlockStateId(1);
const KIND: BlockKindId = BlockKindId(1);

#[derive(Default)]
struct MockRules {
    scheduled_executions: usize,
    scheduled_results: Vec<bool>,
    neighbor_positions: Vec<BlockPos>,
    block_entity_order: Vec<(&'static str, BlockPos)>,
    execute_block_events: bool,
    side_effect_order: Vec<(&'static str, BlockPos)>,
    environment_samples: Vec<(u64, u64, u8)>,
    execution_configs: Vec<ExecutionConfig>,
    pending_execution_config: Option<ExecutionConfig>,
    execution_preparations: Vec<usize>,
    execution_report: ExecutionReport,
    supports_compiled: bool,
    omit_fallback_reason: bool,
    synchronize_follow_up: bool,
    fail_after_action_change: bool,
    synchronized_changes: Vec<Vec<BlockChange>>,
    barrier_order: Vec<&'static str>,
}

struct FallbackWarningCounter(Arc<AtomicUsize>);

impl<S> tracing_subscriber::Layer<S> for FallbackWarningCounter
where
    S: Subscriber,
{
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != Level::WARN {
            return;
        }
        let mut visitor = ReasonFieldVisitor::default();
        event.record(&mut visitor);
        if visitor.has_reason {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct ReasonFieldVisitor {
    has_reason: bool,
}

impl Visit for ReasonFieldVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
        self.has_reason |= field.name() == "reason";
    }
}

impl BlockRules for MockRules {
    fn version(&self) -> &str {
        "test"
    }

    fn air_state(&self) -> BlockStateId {
        AIR
    }

    fn block_kind(&self, state: BlockStateId) -> BlockKindId {
        if state == BLOCK { KIND } else { BlockKindId(0) }
    }

    fn block_name(&self, state: BlockStateId) -> &str {
        if state == BLOCK {
            "test:block"
        } else {
            "test:air"
        }
    }

    fn is_supported(&self, _state: BlockStateId) -> bool {
        true
    }

    fn configure_execution(
        &mut self,
        _world: &SparseWorld,
        config: ExecutionConfig,
    ) -> Result<(), RulesError> {
        self.execution_configs.push(config);
        self.pending_execution_config = Some(config);
        self.execution_report.requested_mode = config.requested_mode;
        if config.requested_mode == ExecutionMode::Compiled
            && (config.trace_enabled || config.redstone_mode == RedstoneMode::Experimental)
        {
            return Err(RulesError::Message(
                "test rules cannot use the compiled backend".to_owned(),
            ));
        }
        Ok(())
    }

    fn prepare_execution(&mut self, world: &SparseWorld) -> Result<(), RulesError> {
        self.execution_preparations.push(world.iter_blocks().count());
        let config = self.pending_execution_config.unwrap_or_default();
        self.execution_report.requested_mode = config.requested_mode;
        self.execution_report.fallback_reason = None;
        if self.supports_compiled
            && !config.trace_enabled
            && config.redstone_mode == RedstoneMode::Default
            && config.requested_mode != ExecutionMode::Interpreted
        {
            self.execution_report.backend = ExecutionBackend::Compiled;
            return Ok(());
        }
        self.execution_report.backend = ExecutionBackend::Interpreted;
        if config.requested_mode == ExecutionMode::Compiled {
            return Err(RulesError::Message(
                "test rules cannot use the compiled backend".to_owned(),
            ));
        }
        if config.requested_mode == ExecutionMode::Auto && !self.omit_fallback_reason {
            self.execution_report.fallback_reason = Some(
                if config.trace_enabled {
                    "test trace requires interpreted execution"
                } else if config.redstone_mode == RedstoneMode::Experimental {
                    "test experimental mode requires interpreted execution"
                } else {
                    "test rules do not support compiled execution"
                }
                .to_owned(),
            );
        }
        Ok(())
    }

    fn synchronize_world(
        &mut self,
        ctx: &mut EventContext<'_>,
        changes: &[BlockChange],
    ) -> Result<(), RulesError> {
        self.barrier_order.push("sync");
        self.synchronized_changes.push(changes.to_vec());
        if self.synchronize_follow_up
            && changes
                .iter()
                .any(|change| change.pos == BlockPos::ZERO && change.new_state == BLOCK)
        {
            let follow_up = BlockPos::new(1, 0, 0);
            ctx.set_block(follow_up, BLOCK, "test_execution_sync")?;
        }
        Ok(())
    }

    fn execution_report(&self) -> ExecutionReport {
        self.execution_report.clone()
    }

    fn initialize(
        &mut self,
        _ctx: &mut EventContext<'_>,
        positions: &[BlockPos],
    ) -> Result<(), RulesError> {
        self.side_effect_order
            .extend(positions.iter().map(|pos| ("initialize", *pos)));
        Ok(())
    }

    fn apply_action(
        &mut self,
        ctx: &mut EventContext<'_>,
        action: &Action,
    ) -> Result<(), RulesError> {
        if let Action::UseBlock { pos } = action {
            ctx.schedule_tick(*pos, KIND, 0, TickPriority::Normal);
        }
        if let Action::PullLever { pos } = action {
            ctx.update_neighbors(*pos, KIND, None, None);
        }
        if let Action::PressButton { pos } = action {
            ctx.neighbor_changed(NeighborUpdate {
                pos: pos.relative(redstone_core::Direction::East),
                source_pos: *pos,
                source_block: KIND,
                orientation: None,
                moved_by_piston: false,
            });
            ctx.schedule_tick_after_neighbors(*pos, KIND, 2, TickPriority::Normal);
        }
        if let Action::BreakBlock { pos } = action {
            ctx.neighbor_changed(NeighborUpdate {
                pos: pos.relative(redstone_core::Direction::East),
                source_pos: *pos,
                source_block: KIND,
                orientation: None,
                moved_by_piston: false,
            });
            ctx.apply_block_changes_after_neighbors(
                vec![DeferredBlockChange {
                    pos: *pos,
                    state: AIR,
                    cause: "deferred_break".to_owned(),
                    block_entity: DeferredBlockEntityUpdate::Remove,
                }],
                Vec::new(),
            );
        }
        if let Action::SetBlockEntity { pos, data } = action {
            ctx.set_block(*pos, BLOCK, "test_block_entity_state")?;
            ctx.set_block_entity(*pos, data.clone());
            if self.fail_after_action_change {
                return Err(RulesError::Message("test action failure".to_owned()));
            }
            if self.synchronize_follow_up {
                ctx.neighbor_changed(NeighborUpdate {
                    pos: pos.relative(redstone_core::Direction::East),
                    source_pos: *pos,
                    source_block: KIND,
                    orientation: None,
                    moved_by_piston: false,
                });
            }
        }
        if let Action::HitTarget { pos, .. } = action {
            ctx.queue_block_event(BlockEvent {
                pos: *pos,
                block: KIND,
                param_a: 2,
                param_b: 5,
            });
        }
        Ok(())
    }

    fn tick_entities(&mut self, ctx: &mut EventContext<'_>) -> Result<(), RulesError> {
        self.environment_samples
            .push((ctx.game_time(), ctx.overworld_time(), ctx.sky_light()));
        Ok(())
    }

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
        _current_state: BlockStateId,
    ) -> Result<(), RulesError> {
        self.barrier_order.push("neighbor");
        self.neighbor_positions.push(update.pos);
        self.side_effect_order.push(("neighbor", update.pos));
        self.block_entity_order.push(("neighbor", update.pos));
        if update.pos == BlockPos::new(-1, 0, 0) {
            ctx.neighbor_changed(NeighborUpdate {
                pos: BlockPos::new(99, 0, 0),
                source_pos: update.pos,
                source_block: KIND,
                orientation: None,
                moved_by_piston: false,
            });
        }
        if update.source_pos == BlockPos::new(99, 0, 0) {
            self.scheduled_results.push(ctx.schedule_tick(
                update.pos,
                KIND,
                2,
                TickPriority::Normal,
            ));
        }
        Ok(())
    }

    fn tick_block_entity(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
    ) -> Result<(), RulesError> {
        self.block_entity_order.push(("tick", pos));
        if ctx
            .world
            .block_entity(pos)
            .is_some_and(|data| data.kind == "test:mutable")
        {
            ctx.update_block_entity(pos, |data| {
                let counter = data
                    .fields
                    .get("counter")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                data.fields
                    .insert("counter".to_owned(), serde_json::Value::from(counter + 1));
            });
        }
        ctx.neighbor_changed(NeighborUpdate {
            pos: pos.relative(redstone_core::Direction::Up),
            source_pos: pos,
            source_block: KIND,
            orientation: None,
            moved_by_piston: false,
        });
        Ok(())
    }

    fn on_scheduled_tick(
        &mut self,
        ctx: &mut EventContext<'_>,
        tick: ScheduledTick,
    ) -> Result<(), RulesError> {
        self.scheduled_executions += 1;
        if self.scheduled_executions == 1 {
            ctx.schedule_tick(tick.pos, tick.block, 0, TickPriority::Normal);
            let neighbor = tick.pos.relative(redstone_core::Direction::East);
            if ctx.world.get_block(neighbor) == BLOCK {
                ctx.neighbor_changed(NeighborUpdate {
                    pos: neighbor,
                    source_pos: BlockPos::new(99, 0, 0),
                    source_block: KIND,
                    orientation: None,
                    moved_by_piston: false,
                });
            }
        }
        Ok(())
    }

    fn on_block_event(
        &mut self,
        ctx: &mut EventContext<'_>,
        event: BlockEvent,
    ) -> Result<bool, RulesError> {
        if !self.execute_block_events {
            return Ok(false);
        }
        ctx.set_block(event.pos, AIR, "test_block_event")?;
        Ok(true)
    }

    fn read_probe(&self, _world: &SparseWorld, _probe: &Probe) -> ProbeValue {
        ProbeValue::Integer(self.scheduled_executions as i64)
    }
}

#[test]
fn region_update_completes_each_position_before_advancing() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation
        .update_region(BlockPos::ZERO, BlockPos::new(1, 0, 0))
        .unwrap();

    let order = &simulation.rules().side_effect_order;
    let second_position = order
        .iter()
        .position(|entry| *entry == ("initialize", BlockPos::new(1, 0, 0)))
        .unwrap();
    assert_eq!(order[0], ("initialize", BlockPos::ZERO));
    assert!(order[1..second_position]
        .iter()
        .any(|entry| *entry == ("neighbor", BlockPos::new(99, 0, 0))));
    assert!(order[1..second_position]
        .iter()
        .all(|(kind, _)| *kind == "neighbor"));
}

#[test]
fn successful_block_event_precedes_its_world_changes() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules {
            execute_block_events: true,
            ..MockRules::default()
        },
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    let delta = simulation
        .step_with_actions(&[Action::HitTarget {
            pos: BlockPos::ZERO,
            face: redstone_core::Direction::East,
            location: [0.5, 0.5, 0.5],
            arrow: false,
        }])
        .unwrap();

    assert_eq!(
        delta.events,
        [
            WorldEvent::BlockEvent {
                event: BlockEvent {
                    pos: BlockPos::ZERO,
                    block: KIND,
                    param_a: 2,
                    param_b: 5,
                },
                block_name: "test:block".to_owned(),
            },
            WorldEvent::Block {
                change: redstone_core::BlockChange {
                    pos: BlockPos::ZERO,
                    old_state: BLOCK,
                    new_state: AIR,
                },
            },
        ]
    );
}

#[test]
fn block_entity_creation_follows_its_block_update() {
    let data = BlockEntityData {
        kind: "test:block_entity".to_owned(),
        fields: BTreeMap::new(),
    };
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();

    let delta = simulation
        .step_with_actions(&[Action::SetBlockEntity {
            pos: BlockPos::ZERO,
            data: data.clone(),
        }])
        .unwrap();

    assert_eq!(
        delta.events,
        [
            WorldEvent::Block {
                change: redstone_core::BlockChange {
                    pos: BlockPos::ZERO,
                    old_state: AIR,
                    new_state: BLOCK,
                },
            },
            WorldEvent::BlockEntity {
                change: BlockEntityChange::Create {
                    pos: BlockPos::ZERO,
                    data,
                },
            },
        ]
    );
}

#[test]
fn block_entity_update_is_recorded_as_an_update() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        BlockEntityData {
            kind: "test:mutable".to_owned(),
            fields: BTreeMap::from([("counter".to_owned(), serde_json::Value::from(0))]),
        },
    );
    let mut simulation = Simulation::load(MockRules::default(), world, SimulationConfig::default())
        .unwrap();

    let delta = simulation.step().unwrap();
    let [
        WorldEvent::BlockEntity {
            change: BlockEntityChange::Update {
                old_data, new_data, ..
            },
        },
    ] = delta.events.as_slice()
    else {
        panic!("expected one block entity update: {:?}", delta.events);
    };
    assert_eq!(old_data.fields["counter"], 0);
    assert_eq!(new_data.fields["counter"], 1);
}

#[test]
fn scheduled_ticks_added_during_execution_wait_for_next_game_tick() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig {
            mode: RedstoneMode::Default,
            trace: true,
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    simulation
        .step_with_actions(&[Action::UseBlock {
            pos: BlockPos::ZERO,
        }])
        .unwrap();
    assert_eq!(simulation.rules().scheduled_executions, 1);
    assert_eq!(simulation.pending_scheduled_ticks(), 1);

    simulation.step().unwrap();
    assert_eq!(simulation.rules().scheduled_executions, 2);
    let executed_ticks = simulation
        .trace()
        .events()
        .iter()
        .filter_map(|event| {
            matches!(event.kind, TraceKind::ScheduledTickExecuted { .. }).then_some(event.tick.0)
        })
        .collect::<Vec<_>>();
    assert_eq!(executed_ticks, vec![1, 2]);
}

#[test]
fn scheduled_tick_remains_deduplicated_until_its_execution_starts() {
    let neighbor = BlockPos::ZERO.relative(redstone_core::Direction::East);
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    world.set_block(neighbor, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation
        .step_with_actions(&[
            Action::UseBlock {
                pos: BlockPos::ZERO,
            },
            Action::UseBlock { pos: neighbor },
        ])
        .unwrap();

    assert_eq!(simulation.rules().scheduled_executions, 2);
    assert_eq!(simulation.rules().scheduled_results, vec![false]);
    assert_eq!(simulation.pending_scheduled_ticks(), 1);
}

#[test]
fn nested_neighbor_update_preempts_multi_update_continuation() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(MockRules::default(), world, SimulationConfig::default())
        .unwrap();

    simulation
        .step_with_actions(&[Action::PullLever {
            pos: BlockPos::ZERO,
        }])
        .unwrap();

    assert_eq!(
        &simulation.rules().neighbor_positions[..3],
        &[
            BlockPos::new(-1, 0, 0),
            BlockPos::new(99, 0, 0),
            BlockPos::new(1, 0, 0),
        ]
    );
}

#[test]
fn block_entity_neighbor_updates_finish_before_the_next_registered_entity_ticks() {
    let first = BlockPos::ZERO;
    let second = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(AIR);
    for pos in [first, second] {
        world.set_block(pos, BLOCK).unwrap();
        world.set_block_entity(
            pos,
            BlockEntityData {
                kind: "test:block_entity".to_owned(),
                fields: BTreeMap::new(),
            },
        );
    }
    let mut simulation = Simulation::load(MockRules::default(), world, SimulationConfig::default())
        .unwrap();

    simulation.step().unwrap();

    assert_eq!(
        simulation.rules().block_entity_order,
        [
            ("tick", first),
            ("neighbor", first.relative(redstone_core::Direction::Up)),
            ("tick", second),
            ("neighbor", second.relative(redstone_core::Direction::Up)),
        ]
    );
}

#[test]
fn deferred_scheduled_tick_is_queued_after_synchronous_neighbor_updates() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig {
            trace: true,
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    simulation
        .step_with_actions(&[Action::PressButton {
            pos: BlockPos::ZERO,
        }])
        .unwrap();

    let ordered = simulation
        .trace()
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::NeighborUpdate { .. } => Some("neighbor"),
            TraceKind::ScheduledTickQueued { .. } => Some("scheduled"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ordered, ["neighbor", "scheduled"]);
}

#[test]
fn deferred_block_changes_are_applied_after_synchronous_neighbor_updates() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let block_entity = BlockEntityData {
        kind: "test:block_entity".to_owned(),
        fields: BTreeMap::new(),
    };
    world.set_block_entity(BlockPos::ZERO, block_entity.clone());
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig {
            trace: true,
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    let delta = simulation
        .step_with_actions(&[Action::BreakBlock {
            pos: BlockPos::ZERO,
        }])
        .unwrap();

    let ordered = simulation
        .trace()
        .events()
        .iter()
        .filter_map(|event| match event.kind {
            TraceKind::NeighborUpdate { .. } => Some("neighbor"),
            TraceKind::BlockChanged { .. } => Some("changed"),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(ordered, ["neighbor", "changed"]);
    assert_eq!(
        simulation.snapshot().world.get_block(BlockPos::ZERO),
        AIR
    );
    assert_eq!(
        delta.events,
        [
            WorldEvent::Block {
                change: redstone_core::BlockChange {
                    pos: BlockPos::ZERO,
                    old_state: BLOCK,
                    new_state: AIR,
                },
            },
            WorldEvent::BlockEntity {
                change: BlockEntityChange::Remove {
                    pos: BlockPos::ZERO,
                    data: block_entity,
                },
            },
        ]
    );
}

#[test]
fn identical_runs_produce_byte_identical_jsonl_and_vcd() {
    fn run() -> (Vec<u8>, Vec<u8>) {
        let mut world = SparseWorld::new(AIR);
        world.set_block(BlockPos::ZERO, BLOCK).unwrap();
        let mut simulation = Simulation::load(
            MockRules::default(),
            world,
            SimulationConfig {
                seed: 42,
                trace: true,
                ..SimulationConfig::default()
            },
        )
        .unwrap();
        simulation.add_probe(
            "counter",
            Probe::EventCount {
                kind: "counter".to_owned(),
            },
        );
        simulation
            .step_with_actions(&[Action::UseBlock {
                pos: BlockPos::ZERO,
            }])
            .unwrap();
        simulation.step().unwrap();
        let trace = simulation.trace();
        let mut jsonl = Vec::new();
        let mut vcd = Vec::new();
        trace.write_jsonl(&mut jsonl).unwrap();
        trace.write_vcd(&mut vcd).unwrap();
        (jsonl, vcd)
    }

    let first = run();
    let second = run();
    assert_eq!(first, second);
}

#[test]
fn monitoring_can_start_after_world_event_recording() {
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig {
            trace: true,
            record_events: true,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    simulation.add_probe(
        "counter",
        Probe::EventCount {
            kind: "counter".to_owned(),
        },
    );
    simulation.set_monitoring_enabled(false);

    let skipped = simulation
        .step_with_actions(&[Action::SetBlockEntity {
            pos: BlockPos::ZERO,
            data: BlockEntityData {
                kind: "test:block_entity".to_owned(),
                fields: BTreeMap::new(),
            },
        }])
        .unwrap();

    assert!(!skipped.events.is_empty());
    assert!(skipped.probes.is_empty());
    assert!(simulation.trace().events().is_empty());

    simulation.set_monitoring_enabled(true);
    let monitored = simulation.step().unwrap();

    assert_eq!(monitored.probes.len(), 1);
    assert!(simulation
        .trace()
        .events()
        .iter()
        .all(|event| event.tick.0 == 2));
}

#[test]
fn environment_time_advances_before_tick_callbacks() {
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig {
            environment: SimulationEnvironment {
                game_time: 10,
                overworld_time: 20,
                advance_time: true,
                sky_light: 7,
            },
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    simulation.step().unwrap();
    simulation.step().unwrap();

    assert_eq!(
        simulation.rules().environment_samples,
        [(11, 21, 7), (12, 22, 7)]
    );
    assert_eq!(simulation.snapshot().environment, simulation.environment());
}

#[test]
fn paused_overworld_clock_does_not_advance() {
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig {
            environment: SimulationEnvironment {
                game_time: 4,
                overworld_time: 30,
                advance_time: false,
                sky_light: 15,
            },
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(simulation.rules().environment_samples, [(5, 30, 15)]);
}

#[test]
fn environment_time_overflow_fails_before_advancing_the_simulation() {
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig {
            environment: SimulationEnvironment {
                game_time: u64::MAX,
                ..SimulationEnvironment::default()
            },
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    assert!(matches!(
        simulation.step(),
        Err(SimulationError::EnvironmentTimeOverflow("game_time"))
    ));
    assert_eq!(simulation.current_tick().0, 0);
}

#[test]
fn execution_configuration_defaults_to_auto() {
    let simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();

    assert_eq!(
        simulation.rules().execution_configs,
        [ExecutionConfig {
            requested_mode: ExecutionMode::Auto,
            redstone_mode: RedstoneMode::Default,
            trace_enabled: false,
            record_events: true,
        }]
    );
    assert_eq!(
        simulation.execution_report().requested_mode,
        ExecutionMode::Auto
    );
    assert_eq!(
        simulation.execution_report().backend,
        ExecutionBackend::Interpreted
    );
}

#[test]
fn forced_compiled_mode_rejects_an_unsupported_rule_set() {
    let mut simulation = Simulation::load(
        MockRules::default(),
        SparseWorld::new(AIR),
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..SimulationConfig::default()
        },
    )
    .unwrap();

    assert!(simulation.rules().execution_preparations.is_empty());
    let result = simulation.prepare_execution();

    assert!(matches!(
        result,
        Err(SimulationError::Rules(RulesError::Message(message)))
            if message == "test rules cannot use the compiled backend"
    ));
}

#[test]
fn auto_mode_supplies_a_fallback_reason_after_preparation() {
    let mut simulation = Simulation::load(
        MockRules {
            omit_fallback_reason: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();

    assert_eq!(simulation.execution_report().fallback_reason, None);
    let warning_count = Arc::new(AtomicUsize::new(0));
    let subscriber = tracing_subscriber::registry()
        .with(FallbackWarningCounter(Arc::clone(&warning_count)));
    tracing::subscriber::with_default(subscriber, || simulation.prepare_execution().unwrap());

    assert!(simulation.execution_report().fallback_reason.is_some());
    assert_eq!(warning_count.load(Ordering::Relaxed), 1);
}

#[test]
fn trace_reconfiguration_rolls_back_a_forced_compiled_backend() {
    let mut simulation = Simulation::load(
        MockRules {
            supports_compiled: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig {
            execution_mode: ExecutionMode::Compiled,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    simulation.prepare_execution().unwrap();

    let result = simulation.set_trace_enabled(true);

    assert!(matches!(result, Err(SimulationError::Rules(_))));
    assert_eq!(
        simulation.execution_report().backend,
        ExecutionBackend::Compiled
    );
    assert_eq!(
        simulation
            .rules()
            .execution_configs
            .iter()
            .map(|config| config.trace_enabled)
            .collect::<Vec<_>>(),
        [false, true, false]
    );
    assert_eq!(simulation.rules().execution_preparations, [0, 0]);
}

#[test]
fn setup_changes_are_applied_before_explicit_execution_preparation() {
    let mut simulation = Simulation::load(
        MockRules {
            supports_compiled: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();
    let mut source = SparseWorld::new(AIR);
    source.set_block(BlockPos::ZERO, BLOCK).unwrap();

    simulation.initialize().unwrap();
    simulation
        .paste_world(
            &source,
            BlockPos::ZERO,
            BlockPos::ZERO,
            false,
            false,
        )
        .unwrap();
    simulation
        .update_region(BlockPos::ZERO, BlockPos::ZERO)
        .unwrap();

    assert!(simulation.rules().execution_preparations.is_empty());
    simulation.prepare_execution().unwrap();
    simulation.prepare_execution().unwrap();
    assert_eq!(simulation.rules().execution_preparations, [1]);
    assert_eq!(
        simulation.execution_report().backend,
        ExecutionBackend::Compiled
    );
}

#[test]
fn step_and_apply_automatically_prepare_execution_once() {
    let mut stepped = Simulation::load(
        MockRules {
            supports_compiled: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();

    stepped.step().unwrap();
    stepped.step().unwrap();
    assert_eq!(stepped.rules().execution_preparations, [0]);

    let mut applied = Simulation::load(
        MockRules {
            supports_compiled: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig::default(),
    )
    .unwrap();

    applied
        .apply(Action::UseBlock {
            pos: BlockPos::ZERO,
        })
        .unwrap();
    assert_eq!(applied.rules().execution_preparations, [0]);
}

#[test]
fn internal_block_changes_synchronize_before_neighbor_tasks_without_recording() {
    let mut simulation = Simulation::load(
        MockRules {
            synchronize_follow_up: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig {
            record_events: false,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    let block_entity = BlockEntityData {
        kind: "test:block_entity".to_owned(),
        fields: BTreeMap::new(),
    };

    let delta = simulation
        .step_with_actions(&[Action::SetBlockEntity {
            pos: BlockPos::ZERO,
            data: block_entity,
        }])
        .unwrap();

    assert!(delta.events.is_empty());
    assert_eq!(simulation.world().get_block(BlockPos::ZERO), BLOCK);
    assert_eq!(
        simulation.world().get_block(BlockPos::new(1, 0, 0)),
        BLOCK
    );
    assert_eq!(
        simulation.rules().synchronized_changes,
        [
            vec![BlockChange {
                pos: BlockPos::ZERO,
                old_state: AIR,
                new_state: BLOCK,
            }],
            vec![BlockChange {
                pos: BlockPos::new(1, 0, 0),
                old_state: AIR,
                new_state: BLOCK,
            }],
        ]
    );
    assert_eq!(
        &simulation.rules().barrier_order[..3],
        ["sync", "sync", "neighbor"]
    );
}

#[test]
fn internal_block_changes_synchronize_even_when_the_rule_callback_fails() {
    let mut simulation = Simulation::load(
        MockRules {
            fail_after_action_change: true,
            ..MockRules::default()
        },
        SparseWorld::new(AIR),
        SimulationConfig {
            record_events: false,
            ..SimulationConfig::default()
        },
    )
    .unwrap();
    let result = simulation.step_with_actions(&[Action::SetBlockEntity {
        pos: BlockPos::ZERO,
        data: BlockEntityData {
            kind: "test:block_entity".to_owned(),
            fields: BTreeMap::new(),
        },
    }]);

    assert!(matches!(
        result,
        Err(SimulationError::Rules(RulesError::Message(message)))
            if message == "test action failure"
    ));
    assert_eq!(simulation.world().get_block(BlockPos::ZERO), BLOCK);
    assert_eq!(
        simulation.rules().synchronized_changes,
        [vec![BlockChange {
            pos: BlockPos::ZERO,
            old_state: AIR,
            new_state: BLOCK,
        }]]
    );
}
