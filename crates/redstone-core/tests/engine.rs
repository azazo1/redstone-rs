use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockEntityData, BlockEvent, BlockKindId, BlockPos, BlockRules, BlockStateId,
    EventContext, NeighborUpdate, Probe, ProbeValue, RedstoneMode, RulesError, ScheduledTick,
    Simulation, SimulationConfig, SparseWorld, TickPriority, TraceKind,
};

const AIR: BlockStateId = BlockStateId(0);
const BLOCK: BlockStateId = BlockStateId(1);
const KIND: BlockKindId = BlockKindId(1);

#[derive(Default)]
struct MockRules {
    scheduled_executions: usize,
    neighbor_positions: Vec<BlockPos>,
    block_entity_order: Vec<(&'static str, BlockPos)>,
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
        if state == BLOCK { "test:block" } else { "test:air" }
    }

    fn is_supported(&self, _state: BlockStateId) -> bool {
        true
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
        Ok(())
    }

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
    ) -> Result<(), RulesError> {
        self.neighbor_positions.push(update.pos);
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
        Ok(())
    }

    fn tick_block_entity(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
    ) -> Result<(), RulesError> {
        self.block_entity_order.push(("tick", pos));
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
        }
        Ok(())
    }

    fn on_block_event(
        &mut self,
        _ctx: &mut EventContext<'_>,
        _event: BlockEvent,
    ) -> Result<(), RulesError> {
        Ok(())
    }

    fn read_probe(&self, _world: &SparseWorld, _probe: &Probe) -> ProbeValue {
        ProbeValue::Integer(self.scheduled_executions as i64)
    }
}

#[tokio::test]
async fn scheduled_ticks_added_during_execution_wait_for_next_game_tick() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig {
            mode: RedstoneMode::Default,
            ..SimulationConfig::default()
        },
    )
    .await
    .unwrap();

    simulation
        .step_with_actions(&[Action::UseBlock { pos: BlockPos::ZERO }])
        .await
        .unwrap();
    assert_eq!(simulation.rules().scheduled_executions, 1);
    assert_eq!(simulation.pending_scheduled_ticks(), 1);

    simulation.step().await.unwrap();
    assert_eq!(simulation.rules().scheduled_executions, 2);
    let executed_ticks = simulation
        .trace()
        .events()
        .iter()
        .filter_map(|event| {
            matches!(event.kind, TraceKind::ScheduledTickExecuted { .. })
                .then_some(event.tick.0)
        })
        .collect::<Vec<_>>();
    assert_eq!(executed_ticks, vec![1, 2]);
}

#[tokio::test]
async fn nested_neighbor_update_preempts_multi_update_continuation() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig::default(),
    )
    .await
    .unwrap();

    simulation
        .step_with_actions(&[Action::PullLever { pos: BlockPos::ZERO }])
        .await
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

#[tokio::test]
async fn block_entity_neighbor_updates_finish_before_the_next_registered_entity_ticks() {
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
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig::default(),
    )
    .await
    .unwrap();

    simulation.step().await.unwrap();

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

#[tokio::test]
async fn deferred_scheduled_tick_is_queued_after_synchronous_neighbor_updates() {
    let mut world = SparseWorld::new(AIR);
    world.set_block(BlockPos::ZERO, BLOCK).unwrap();
    let mut simulation = Simulation::load(
        MockRules::default(),
        world,
        SimulationConfig::default(),
    )
    .await
    .unwrap();

    simulation
        .step_with_actions(&[Action::PressButton { pos: BlockPos::ZERO }])
        .await
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

#[tokio::test]
async fn identical_runs_produce_byte_identical_jsonl_and_vcd() {
    async fn run() -> (Vec<u8>, Vec<u8>) {
        let mut world = SparseWorld::new(AIR);
        world.set_block(BlockPos::ZERO, BLOCK).unwrap();
        let mut simulation = Simulation::load(
            MockRules::default(),
            world,
            SimulationConfig {
                seed: 42,
                ..SimulationConfig::default()
            },
        )
        .await
        .unwrap();
        simulation.add_probe("counter", Probe::EventCount { kind: "counter".to_owned() });
        simulation
            .step_with_actions(&[Action::UseBlock { pos: BlockPos::ZERO }])
            .await
            .unwrap();
        simulation.step().await.unwrap();
        let trace = simulation.trace();
        let mut jsonl = Vec::new();
        let mut vcd = Vec::new();
        trace.write_jsonl(&mut jsonl).unwrap();
        trace.write_vcd(&mut vcd).unwrap();
        (jsonl, vcd)
    }

    let first = run().await;
    let second = run().await;
    assert_eq!(first, second);
}
