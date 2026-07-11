use std::collections::{BTreeSet, VecDeque};

use indexmap::{IndexMap, IndexSet};
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{debug, warn};
use tracing_indicatif::{span_ext::IndicatifSpanExt, style::ProgressStyle};

use crate::{
    Action, BlockEvent, BlockKindId, BlockPos, BlockRules, DeferredBlockEntityUpdate, Direction,
    EventContext, GameTick, MicroStep, NeighborTask, NeighborUpdate, Probe, ProbeSample,
    RedstoneMode, RulesError, ScheduledTick, SimulationPhase, SparseWorld, TraceEvent, TraceKind,
    TraceLog, WorldDelta, WorldEvent,
};
use crate::rules::NeighborTasks;

const DEFAULT_MAX_SCHEDULED_TICKS_PER_TICK: usize = 65_536;
const DEFAULT_MAX_CHAINED_NEIGHBOR_UPDATES: usize = 1_000_000;
const UPDATE_REGION_PROGRESS_INTERVAL: usize = 16_384;

fn region_update_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{span_child_prefix}{spinner:.green} {msg} [{bar:28.green}] {pos}/{len} {per_sec:2} ETA:{eta}",
    )
    .expect("选区更新进度条模板必须有效")
    .progress_chars("=> ")
}

#[derive(Clone, Debug)]
pub struct SimulationConfig {
    pub mode: RedstoneMode,
    pub seed: u64,
    pub strict: bool,
    pub trace: bool,
    pub record_events: bool,
    pub max_scheduled_ticks_per_tick: usize,
    pub max_chained_neighbor_updates: usize,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            mode: RedstoneMode::Default,
            seed: 0,
            strict: true,
            trace: false,
            record_events: true,
            max_scheduled_ticks_per_tick: DEFAULT_MAX_SCHEDULED_TICKS_PER_TICK,
            max_chained_neighbor_updates: DEFAULT_MAX_CHAINED_NEIGHBOR_UPDATES,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub tick: GameTick,
    pub world: SparseWorld,
}

#[derive(Clone, Copy)]
pub struct WorldPaste<'a> {
    pub source: &'a SparseWorld,
    pub region_min: BlockPos,
    pub region_max: BlockPos,
    pub ignore_air: bool,
    pub paste_entities: bool,
    pub update: bool,
}

pub struct Simulation<R: BlockRules> {
    rules: R,
    world: SparseWorld,
    config: SimulationConfig,
    tick: GameTick,
    micro_step: MicroStep,
    next_sub_tick_order: i64,
    random_state: u64,
    scheduled_ticks: BTreeSet<ScheduledTick>,
    scheduled_keys: BTreeSet<(BlockPos, BlockKindId)>,
    block_events: VecDeque<BlockEvent>,
    block_event_keys: BTreeSet<BlockEvent>,
    trace: Vec<TraceEvent>,
    probes: IndexMap<String, Probe>,
    tickable_block_entities: IndexSet<BlockPos>,
    delta_tx: broadcast::Sender<WorldDelta>,
}

impl<R: BlockRules> Simulation<R> {
    pub fn load(
        mut rules: R,
        mut world: SparseWorld,
        config: SimulationConfig,
    ) -> Result<Self, SimulationError> {
        if world.air() != rules.air_state() {
            return Err(SimulationError::AirStateMismatch);
        }
        world.optimize_section_access();
        if config.strict {
            let unsupported = world
                .iter_blocks()
                .filter(|(_, state)| !rules.is_supported(*state))
                .map(|(pos, state)| (pos, rules.block_name(state).to_owned()))
                .collect::<Vec<_>>();
            if !unsupported.is_empty() {
                return Err(SimulationError::UnsupportedBlocks(unsupported));
            }
        }
        rules.load_world(&world)?;
        let tickable_block_entities = world
            .block_entities()
            .filter(|(pos, data)| rules.should_tick_block_entity(&world, **pos, data))
            .map(|(pos, _)| *pos)
            .collect();
        let (delta_tx, _) = broadcast::channel(256);
        let random_state = (config.seed ^ 0x5deece66d) & ((1 << 48) - 1);
        Ok(Self {
            rules,
            world,
            config,
            tick: GameTick(0),
            micro_step: MicroStep(0),
            next_sub_tick_order: 0,
            random_state,
            scheduled_ticks: BTreeSet::new(),
            scheduled_keys: BTreeSet::new(),
            block_events: VecDeque::new(),
            block_event_keys: BTreeSet::new(),
            trace: Vec::new(),
            probes: IndexMap::new(),
            tickable_block_entities,
            delta_tx,
        })
    }

    pub fn world(&self) -> &SparseWorld {
        &self.world
    }

    pub fn rules(&self) -> &R {
        &self.rules
    }

    pub fn current_tick(&self) -> GameTick {
        self.tick
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.config.trace = enabled;
        if !enabled {
            self.trace.clear();
        }
    }

    pub fn initialize(&mut self) -> Result<(), SimulationError> {
        let positions = self
            .world
            .iter_blocks()
            .map(|(pos, _)| pos)
            .collect::<Vec<_>>();
        let tasks = self.with_context(SimulationPhase::PreTick, |rules, ctx| {
            rules.initialize(ctx, &positions)
        })?;
        self.process_neighbor_tasks(tasks, SimulationPhase::PreTick)?;
        Ok(())
    }

    pub fn paste_world(
        &mut self,
        source: &SparseWorld,
        region_min: BlockPos,
        region_max: BlockPos,
        ignore_air: bool,
        paste_entities: bool,
    ) -> Result<(), SimulationError> {
        let paste = WorldPaste {
            source,
            region_min,
            region_max,
            ignore_air,
            paste_entities,
            update: false,
        };
        self.paste_world_with_changes(paste, &mut Vec::new())
    }

    fn paste_world_with_changes(
        &mut self,
        paste: WorldPaste<'_>,
        changes: &mut Vec<WorldEvent>,
    ) -> Result<(), SimulationError> {
        let source = paste.source;
        let (region_min, region_max) = ordered_bounds(paste.region_min, paste.region_max);
        let blocks = if paste.ignore_air {
            source
                .iter_blocks()
                .filter(|(pos, _)| position_in_region(*pos, region_min, region_max))
                .collect::<Vec<_>>()
        } else {
            region_positions(region_min, region_max)
                .map(|pos| (pos, source.get_block(pos)))
                .collect::<Vec<_>>()
        };
        if self.config.strict {
            let unsupported = blocks
                .iter()
                .filter(|(_, state)| !self.rules.is_supported(*state))
                .map(|(pos, state)| (*pos, self.rules.block_name(*state).to_owned()))
                .collect::<Vec<_>>();
            if !unsupported.is_empty() {
                return Err(SimulationError::UnsupportedBlocks(unsupported));
            }
        }
        let block_entities = source
            .block_entities()
            .filter(|(pos, _)| {
                position_in_region(**pos, region_min, region_max)
                    && (!paste.ignore_air || source.get_block(**pos) != source.air())
            })
            .map(|(pos, data)| (*pos, data.clone()))
            .collect::<Vec<_>>();
        let entities = if paste.paste_entities {
            source
                .entities()
                .map(|(_, entity)| entity.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let tasks = self.with_context_and_changes(
            SimulationPhase::PreTick,
            changes,
            |_rules, ctx| {
                for (pos, state) in blocks {
                    ctx.set_block(pos, state, "structure_paste")?;
                    ctx.remove_block_entity(pos);
                }
                for (pos, data) in block_entities {
                    ctx.set_block_entity(pos, data);
                }
                for entity in entities {
                    ctx.world.spawn_entity(entity);
                }
                Ok(())
            },
        )?;
        self.process_neighbor_tasks_with_changes(tasks, SimulationPhase::PreTick, changes)?;
        self.rules.load_world(&self.world)?;
        Ok(())
    }

    pub fn update_region(
        &mut self,
        region_min: BlockPos,
        region_max: BlockPos,
    ) -> Result<(), SimulationError> {
        self.update_region_with_changes(region_min, region_max, &mut Vec::new())
    }

    fn update_region_with_changes(
        &mut self,
        region_min: BlockPos,
        region_max: BlockPos,
        changes: &mut Vec<WorldEvent>,
    ) -> Result<(), SimulationError> {
        let (region_min, region_max) = ordered_bounds(region_min, region_max);
        let positions = region_positions(region_min, region_max).collect::<Vec<_>>();
        let progress = tracing::info_span!("region_update", ?region_min, ?region_max);
        progress.pb_set_style(&region_update_progress_style());
        progress.pb_set_length(positions.len() as u64);
        progress.pb_set_message("更新粘贴选区");
        progress.pb_start();
        for (index, pos) in positions.iter().copied().enumerate() {
            let tasks = self.with_context_and_changes(
                SimulationPhase::PreTick,
                changes,
                |rules, ctx| rules.apply_update_side_effects(ctx, pos),
            )?;
            self.process_neighbor_tasks_with_changes(tasks, SimulationPhase::PreTick, changes)?;
            let processed = index + 1;
            if processed % UPDATE_REGION_PROGRESS_INTERVAL == 0 {
                progress.pb_set_position(processed as u64);
            }
        }
        progress.pb_set_position(positions.len() as u64);
        Ok(())
    }

    pub fn apply(&mut self, action: Action) -> Result<WorldDelta, SimulationError> {
        let mut changes = Vec::new();
        self.push_trace(
            SimulationPhase::PreTick,
            TraceKind::Action {
                action: action.clone(),
            },
        );
        let tasks =
            self.with_context_and_changes(SimulationPhase::PreTick, &mut changes, |rules, ctx| {
                rules.apply_action(ctx, &action)
            })?;
        self.process_neighbor_tasks_with_changes(tasks, SimulationPhase::PreTick, &mut changes)?;
        let delta = WorldDelta {
            tick: self.tick,
            events: changes,
            probes: Vec::new(),
        };
        if self.delta_tx.receiver_count() > 0 {
            let _ = self.delta_tx.send(delta.clone());
        }
        Ok(delta)
    }

    pub fn step(&mut self) -> Result<WorldDelta, SimulationError> {
        self.step_with_actions(&[])
    }

    pub fn step_with_actions(
        &mut self,
        actions: &[Action],
    ) -> Result<WorldDelta, SimulationError> {
        self.step_with_pastes_and_actions(&[], actions)
    }

    pub fn step_with_pastes_and_actions(
        &mut self,
        pastes: &[WorldPaste<'_>],
        actions: &[Action],
    ) -> Result<WorldDelta, SimulationError> {
        self.tick.0 += 1;
        self.micro_step = MicroStep(0);
        self.rules.begin_tick(self.tick);
        let mut changes = Vec::new();

        for paste in pastes {
            self.paste_world_with_changes(*paste, &mut changes)?;
            if paste.update {
                self.update_region_with_changes(
                    paste.region_min,
                    paste.region_max,
                    &mut changes,
                )?;
            }
        }

        for action in actions {
            self.push_trace(
                SimulationPhase::PreTick,
                TraceKind::Action {
                    action: action.clone(),
                },
            );
            let tasks = self.with_context_and_changes(
                SimulationPhase::PreTick,
                &mut changes,
                |rules, ctx| rules.apply_action(ctx, action),
            )?;
            self.process_neighbor_tasks_with_changes(
                tasks,
                SimulationPhase::PreTick,
                &mut changes,
            )?;
        }

        self.run_scheduled_ticks(&mut changes)?;
        self.run_block_events(&mut changes)?;

        let tasks = self.with_context_and_changes(
            SimulationPhase::Entities,
            &mut changes,
            |rules, ctx| rules.tick_entities(ctx),
        )?;
        self.process_neighbor_tasks_with_changes(tasks, SimulationPhase::Entities, &mut changes)?;

        self.run_block_entities(&mut changes)?;

        let probes = self.sample_probes();
        let delta = WorldDelta {
            tick: self.tick,
            events: changes,
            probes,
        };
        if self.delta_tx.receiver_count() > 0 {
            let _ = self.delta_tx.send(delta.clone());
        }
        Ok(delta)
    }

    pub fn run_until(
        &mut self,
        target: GameTick,
    ) -> Result<Vec<WorldDelta>, SimulationError> {
        let mut deltas = Vec::new();
        while self.tick < target {
            deltas.push(self.step()?);
        }
        Ok(deltas)
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            tick: self.tick,
            world: self.world.clone(),
        }
    }

    pub fn add_probe(&mut self, name: impl Into<String>, probe: Probe) {
        self.probes.insert(name.into(), probe);
    }

    pub fn subscribe_deltas(&self) -> broadcast::Receiver<WorldDelta> {
        self.delta_tx.subscribe()
    }

    pub fn trace(&self) -> TraceLog<'_> {
        TraceLog::borrowed(&self.trace)
    }

    pub fn pending_scheduled_ticks(&self) -> usize {
        self.scheduled_ticks.len()
    }

    fn run_scheduled_ticks(
        &mut self,
        changes: &mut Vec<WorldEvent>,
    ) -> Result<(), SimulationError> {
        let due = self
            .scheduled_ticks
            .iter()
            .take_while(|tick| tick.trigger_tick <= self.tick)
            .take(self.config.max_scheduled_ticks_per_tick)
            .copied()
            .collect::<Vec<_>>();
        for tick in &due {
            self.scheduled_ticks.remove(tick);
            self.scheduled_keys.remove(&(tick.pos, tick.block));
        }
        if self
            .scheduled_ticks
            .first()
            .is_some_and(|tick| tick.trigger_tick <= self.tick)
        {
            warn!(
                tick = self.tick.0,
                limit = self.config.max_scheduled_ticks_per_tick,
                "计划方块刻达到单刻上限"
            );
        }
        for tick in due {
            self.push_trace(
                SimulationPhase::ScheduledTicks,
                TraceKind::ScheduledTickExecuted {
                    pos: tick.pos,
                    block: tick.block,
                },
            );
            let tasks = self.with_context_and_changes(
                SimulationPhase::ScheduledTicks,
                changes,
                |rules, ctx| rules.on_scheduled_tick(ctx, tick),
            )?;
            self.process_neighbor_tasks_with_changes(
                tasks,
                SimulationPhase::ScheduledTicks,
                changes,
            )?;
        }
        Ok(())
    }

    fn run_block_events(&mut self, changes: &mut Vec<WorldEvent>) -> Result<(), SimulationError> {
        while let Some(event) = self.block_events.pop_front() {
            self.block_event_keys.remove(&event);
            let block_name = self
                .rules
                .block_name(self.world.get_block(event.pos))
                .to_owned();
            self.push_trace(
                SimulationPhase::BlockEvents,
                TraceKind::BlockEventExecuted {
                    pos: event.pos,
                    block: event.block,
                    param_a: event.param_a,
                    param_b: event.param_b,
                },
            );
            let event_index = changes.len();
            let mut executed = false;
            let tasks = self.with_context_and_changes(
                SimulationPhase::BlockEvents,
                changes,
                |rules, ctx| {
                    executed = rules.on_block_event(ctx, event)?;
                    Ok(())
                },
            )?;
            self.process_neighbor_tasks_with_changes(tasks, SimulationPhase::BlockEvents, changes)?;
            if executed {
                changes.insert(event_index, WorldEvent::BlockEvent { event, block_name });
            }
        }
        Ok(())
    }

    fn run_block_entities(&mut self, changes: &mut Vec<WorldEvent>) -> Result<(), SimulationError> {
        let positions = self
            .tickable_block_entities
            .iter()
            .copied()
            .collect::<Vec<_>>();
        for pos in positions {
            if self.world.block_entity(pos).is_none() {
                continue;
            }
            let tasks = self.with_context_and_changes(
                SimulationPhase::BlockEntities,
                changes,
                |rules, ctx| rules.tick_block_entity(ctx, pos),
            )?;
            self.process_neighbor_tasks_with_changes(
                tasks,
                SimulationPhase::BlockEntities,
                changes,
            )?;
        }
        Ok(())
    }

    fn process_neighbor_tasks(
        &mut self,
        tasks: NeighborTasks,
        phase: SimulationPhase,
    ) -> Result<(), SimulationError> {
        self.process_neighbor_tasks_with_changes(tasks, phase, &mut Vec::new())
    }

    fn process_neighbor_tasks_with_changes(
        &mut self,
        mut tasks: NeighborTasks,
        phase: SimulationPhase,
        changes: &mut Vec<WorldEvent>,
    ) -> Result<(), SimulationError> {
        tasks.reverse();
        let mut stack = tasks;
        let mut count = 0usize;
        while let Some(task) = stack.pop() {
            let task = match task {
                NeighborTask::ScheduleTickAfterNeighbors {
                    pos,
                    block,
                    delay,
                    priority,
                } => {
                    self.append_context_tasks(
                        phase,
                        changes,
                        &mut stack,
                        |_rules, ctx| {
                            ctx.schedule_tick(pos, block, delay, priority);
                            Ok(())
                        },
                    )?;
                    continue;
                }
                NeighborTask::SetBlockAndUpdateNeighborsAfterNeighbors {
                    pos,
                    state,
                    cause,
                    source_block,
                } => {
                    self.append_context_tasks(
                        phase,
                        changes,
                        &mut stack,
                        |_rules, ctx| {
                            let old = ctx.set_block(pos, state, cause)?;
                            if old != state {
                                ctx.update_neighbors(pos, source_block, None, None);
                            }
                            Ok(())
                        },
                    )?;
                    continue;
                }
                NeighborTask::ApplyBlockChangesAfterNeighbors {
                    changes: deferred_changes,
                    follow_up,
                } => {
                    for task in follow_up.into_iter().rev() {
                        stack.push(task);
                    }
                    self.append_context_tasks(
                        phase,
                        changes,
                        &mut stack,
                        |_rules, ctx| {
                            for change in deferred_changes {
                                ctx.set_block(change.pos, change.state, change.cause)?;
                                match change.block_entity {
                                    DeferredBlockEntityUpdate::Keep => {}
                                    DeferredBlockEntityUpdate::Remove => {
                                        ctx.remove_block_entity(change.pos);
                                    }
                                    DeferredBlockEntityUpdate::Set(data) => {
                                        ctx.set_block_entity(change.pos, data);
                                    }
                                }
                            }
                            Ok(())
                        },
                    )?;
                    continue;
                }
                NeighborTask::RunRuleTaskAfterNeighbors(task) => {
                    self.append_context_tasks(
                        phase,
                        changes,
                        &mut stack,
                        |rules, ctx| rules.on_deferred_task(ctx, task),
                    )?;
                    continue;
                }
                task => task,
            };
            count += 1;
            if count > self.config.max_chained_neighbor_updates {
                warn!(
                    tick = self.tick.0,
                    limit = self.config.max_chained_neighbor_updates,
                    "连锁邻居更新达到上限"
                );
                break;
            }

            let (update, continuation) = match task {
                NeighborTask::Single(update) => (update, None),
                NeighborTask::Multi {
                    source_pos,
                    source_block,
                    skip_direction,
                    orientation,
                    mut next_index,
                } => {
                    while next_index < Direction::UPDATE_ORDER.len()
                        && Some(Direction::UPDATE_ORDER[next_index]) == skip_direction
                    {
                        next_index += 1;
                    }
                    if next_index >= Direction::UPDATE_ORDER.len() {
                        continue;
                    }
                    let direction = Direction::UPDATE_ORDER[next_index];
                    let update = NeighborUpdate {
                        pos: source_pos.relative(direction),
                        source_pos,
                        source_block,
                        orientation,
                        moved_by_piston: false,
                    };
                    next_index += 1;
                    let continuation = (next_index < Direction::UPDATE_ORDER.len()).then_some(
                        NeighborTask::Multi {
                            source_pos,
                            source_block,
                            skip_direction,
                            orientation,
                            next_index,
                        },
                    );
                    (update, continuation)
                }
                NeighborTask::ScheduleTickAfterNeighbors { .. } => unreachable!(),
                NeighborTask::SetBlockAndUpdateNeighborsAfterNeighbors { .. } => unreachable!(),
                NeighborTask::ApplyBlockChangesAfterNeighbors { .. } => unreachable!(),
                NeighborTask::RunRuleTaskAfterNeighbors(_) => unreachable!(),
            };

            if self.config.trace {
                self.push_trace(
                    phase,
                    TraceKind::NeighborUpdate {
                        pos: update.pos,
                        source_pos: update.source_pos,
                        source_block: update.source_block,
                        moved_by_piston: update.moved_by_piston,
                        orientation: update.orientation,
                    },
                );
            }
            if let Some(continuation) = continuation {
                stack.push(continuation);
            }
            self.append_context_tasks(
                phase,
                changes,
                &mut stack,
                |rules, ctx| rules.on_neighbor_update(ctx, update),
            )?;
        }
        debug!(tick = self.tick.0, count, "完成邻居更新链");
        Ok(())
    }

    fn sample_probes(&mut self) -> Vec<ProbeSample> {
        let samples = self
            .probes
            .iter()
            .map(|(name, probe)| ProbeSample {
                name: name.clone(),
                probe: probe.clone(),
                value: self.rules.read_probe(&self.world, probe),
            })
            .collect::<Vec<_>>();
        for sample in &samples {
            self.push_trace(
                SimulationPhase::PostTick,
                TraceKind::ProbeSample {
                    sample: sample.clone(),
                },
            );
        }
        samples
    }

    fn with_context<T>(
        &mut self,
        phase: SimulationPhase,
        callback: impl FnOnce(&mut R, &mut EventContext<'_>) -> Result<T, RulesError>,
    ) -> Result<NeighborTasks, SimulationError> {
        self.with_context_and_changes(phase, &mut Vec::new(), callback)
    }

    fn with_context_and_changes<T>(
        &mut self,
        phase: SimulationPhase,
        changes: &mut Vec<WorldEvent>,
        callback: impl FnOnce(&mut R, &mut EventContext<'_>) -> Result<T, RulesError>,
    ) -> Result<NeighborTasks, SimulationError> {
        let mut tasks = NeighborTasks::new();
        self.with_context_and_changes_into_tasks(phase, changes, &mut tasks, callback)?;
        Ok(tasks)
    }

    fn append_context_tasks<T>(
        &mut self,
        phase: SimulationPhase,
        changes: &mut Vec<WorldEvent>,
        tasks: &mut NeighborTasks,
        callback: impl FnOnce(&mut R, &mut EventContext<'_>) -> Result<T, RulesError>,
    ) -> Result<(), SimulationError> {
        let start = tasks.len();
        self.with_context_and_changes_into_tasks(phase, changes, tasks, callback)?;
        tasks[start..].reverse();
        Ok(())
    }

    fn with_context_and_changes_into_tasks<T>(
        &mut self,
        phase: SimulationPhase,
        changes: &mut Vec<WorldEvent>,
        tasks: &mut NeighborTasks,
        callback: impl FnOnce(&mut R, &mut EventContext<'_>) -> Result<T, RulesError>,
    ) -> Result<(), SimulationError> {
        let trace = self.config.trace.then_some(&mut self.trace);
        let events = self.config.record_events.then_some(changes);
        let mut ctx = EventContext::new(
            &mut self.world,
            self.config.mode,
            self.tick,
            phase,
            &mut self.micro_step,
            &mut self.next_sub_tick_order,
            &mut self.random_state,
            &mut self.scheduled_ticks,
            &mut self.scheduled_keys,
            &mut self.block_events,
            &mut self.block_event_keys,
            trace,
            events,
            tasks,
        );
        callback(&mut self.rules, &mut ctx)?;
        let affected = ctx.take_touched_block_entities();
        drop(ctx);
        for pos in affected {
            let tickable = self.world.block_entity(pos).is_some_and(|data| {
                self.rules
                    .should_tick_block_entity(&self.world, pos, data)
            });
            if tickable {
                self.tickable_block_entities.insert(pos);
            } else {
                self.tickable_block_entities.shift_remove(&pos);
            }
        }
        Ok(())
    }

    fn push_trace(&mut self, phase: SimulationPhase, kind: TraceKind) {
        if !self.config.trace {
            return;
        }
        self.micro_step.0 += 1;
        self.trace.push(TraceEvent {
            tick: self.tick,
            micro_step: self.micro_step,
            phase,
            kind,
        });
    }
}

fn ordered_bounds(first: BlockPos, second: BlockPos) -> (BlockPos, BlockPos) {
    (
        BlockPos::new(
            first.x.min(second.x),
            first.y.min(second.y),
            first.z.min(second.z),
        ),
        BlockPos::new(
            first.x.max(second.x),
            first.y.max(second.y),
            first.z.max(second.z),
        ),
    )
}

fn position_in_region(pos: BlockPos, min: BlockPos, max: BlockPos) -> bool {
    (min.x..=max.x).contains(&pos.x)
        && (min.y..=max.y).contains(&pos.y)
        && (min.z..=max.z).contains(&pos.z)
}

fn region_positions(
    min: BlockPos,
    max: BlockPos,
) -> impl Iterator<Item = BlockPos> {
    (min.y..=max.y).flat_map(move |y| {
        (min.z..=max.z)
            .flat_map(move |z| (min.x..=max.x).map(move |x| BlockPos::new(x, y, z)))
    })
}

#[derive(Debug, Error)]
pub enum SimulationError {
    #[error(transparent)]
    Rules(#[from] RulesError),
    #[error("世界空气状态与规则集不一致")]
    AirStateMismatch,
    #[error("严格模式发现未支持方块: {0:?}")]
    UnsupportedBlocks(Vec<(BlockPos, String)>),
}
