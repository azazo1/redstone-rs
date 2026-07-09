use redstone_core::{
    Action, BlockEvent, BlockKindId, BlockPos, BlockRules, BlockStateId, EventContext,
    NeighborUpdate, Probe, ProbeValue, RedstoneMode, RulesError, ScheduledTick, Simulation,
    SimulationConfig, SparseWorld, TickPriority, TraceKind,
};

const AIR: BlockStateId = BlockStateId(0);
const BLOCK: BlockStateId = BlockStateId(1);
const KIND: BlockKindId = BlockKindId(1);

#[derive(Default)]
struct MockRules {
    scheduled_executions: usize,
    neighbor_positions: Vec<BlockPos>,
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
        Ok(())
    }

    fn on_neighbor_update(
        &mut self,
        ctx: &mut EventContext<'_>,
        update: NeighborUpdate,
    ) -> Result<(), RulesError> {
        self.neighbor_positions.push(update.pos);
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
