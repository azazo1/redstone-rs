use std::collections::{BTreeSet, VecDeque};

use indexmap::{IndexMap, IndexSet};
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{debug, warn};
use tracing_indicatif::{span_ext::IndicatifSpanExt, style::ProgressStyle};

use crate::{
    Action, BlockChange, BlockEvent, BlockPos, BlockRules, DeferredBlockEntityUpdate, Direction,
    EventContext, ExecutionBackend, ExecutionConfig, ExecutionMode, ExecutionReport, GameTick,
    MicroStep, NeighborTask, NeighborUpdate, Probe, ProbeSample, RedstoneMode, RulesError,
    SimulationEnvironment, SimulationPhase, SparseWorld, TraceEvent, TraceKind, TraceLog,
    WorldDelta, WorldEvent,
};
use crate::rules::NeighborTasks;
use crate::scheduler::ScheduledTickQueue;

const DEFAULT_MAX_SCHEDULED_TICKS_PER_TICK: usize = 65_536;
const DEFAULT_MAX_CHAINED_NEIGHBOR_UPDATES: usize = 1_000_000;
const MAX_EXECUTION_SYNCHRONIZATION_PASSES: usize = 65_536;
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
    pub execution_mode: ExecutionMode,
    pub seed: u64,
    pub environment: SimulationEnvironment,
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
            execution_mode: ExecutionMode::Auto,
            seed: 0,
            environment: SimulationEnvironment::default(),
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
    pub environment: SimulationEnvironment,
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
    execution_prepared: bool,
    tick: GameTick,
    game_time: u64,
    overworld_time: u64,
    micro_step: MicroStep,
    next_sub_tick_order: i64,
    random_state: u64,
    scheduled_ticks: ScheduledTickQueue,
    block_events: VecDeque<BlockEvent>,
    block_event_keys: BTreeSet<BlockEvent>,
    execution_block_changes: Vec<BlockChange>,
    trace: Vec<TraceEvent>,
    monitoring_enabled: bool,
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
        rules.configure_execution(
            &world,
            ExecutionConfig {
                requested_mode: config.execution_mode,
                redstone_mode: config.mode,
                trace_enabled: config.trace,
                record_events: config.record_events,
            },
        )?;
        let tickable_block_entities = world
            .block_entities()
            .filter(|(pos, data)| rules.should_tick_block_entity(&world, **pos, data))
            .map(|(pos, _)| *pos)
            .collect();
        let (delta_tx, _) = broadcast::channel(256);
        let random_state = (config.seed ^ 0x5deece66d) & ((1 << 48) - 1);
        let game_time = config.environment.game_time;
        let overworld_time = config.environment.overworld_time;
        Ok(Self {
            rules,
            world,
            config,
            execution_prepared: false,
            tick: GameTick(0),
            game_time,
            overworld_time,
            micro_step: MicroStep(0),
            next_sub_tick_order: 0,
            random_state,
            scheduled_ticks: ScheduledTickQueue::new(),
            block_events: VecDeque::new(),
            block_event_keys: BTreeSet::new(),
            execution_block_changes: Vec::new(),
            trace: Vec::new(),
            monitoring_enabled: true,
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

    pub fn environment(&self) -> SimulationEnvironment {
        SimulationEnvironment {
            game_time: self.game_time,
            overworld_time: self.overworld_time,
            advance_time: self.config.environment.advance_time,
            sky_light: self.config.environment.sky_light,
        }
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) -> Result<(), SimulationError> {
        let previous = self.config.trace;
        if previous == enabled {
            return Ok(());
        }
        let was_prepared = self.execution_prepared;
        self.config.trace = enabled;
        let configured = self.rules.configure_execution(
            &self.world,
            ExecutionConfig {
                requested_mode: self.config.execution_mode,
                redstone_mode: self.config.mode,
                trace_enabled: enabled,
                record_events: self.config.record_events,
            },
        );
        self.execution_prepared = false;
        let configured = configured
            .map_err(SimulationError::from)
            .and_then(|()| {
                if was_prepared {
                    self.prepare_execution()
                } else {
                    Ok(())
                }
            });
        if let Err(error) = configured {
            self.config.trace = previous;
            self.execution_prepared = false;
            let rollback = ExecutionConfig {
                requested_mode: self.config.execution_mode,
                redstone_mode: self.config.mode,
                trace_enabled: previous,
                record_events: self.config.record_events,
            };
            if let Err(rollback_error) = self.rules.configure_execution(&self.world, rollback) {
                warn!(%rollback_error, "恢复执行器诊断配置失败");
            } else if was_prepared
                && let Err(rollback_error) = self.prepare_execution()
            {
                warn!(%rollback_error, "恢复执行器诊断状态失败");
            }
            return Err(error);
        }
        if !enabled {
            self.trace.clear();
        }
        Ok(())
    }

    pub fn prepare_execution(&mut self) -> Result<(), SimulationError> {
        if self.execution_prepared {
            return Ok(());
        }
        self.rules.prepare_execution(&self.world)?;
        validate_execution_report(
            self.config.execution_mode,
            &normalized_execution_report(&self.rules, &self.config, true),
        )?;
        self.execution_prepared = true;
        Ok(())
    }

    pub fn set_monitoring_enabled(&mut self, enabled: bool) {
        self.monitoring_enabled = enabled;
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
        self.prepare_execution()?;
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
        self.prepare_execution()?;
        let next_tick = self
            .tick
            .0
            .checked_add(1)
            .ok_or(SimulationError::EnvironmentTimeOverflow("simulation tick"))?;
        let next_game_time = self
            .game_time
            .checked_add(1)
            .ok_or(SimulationError::EnvironmentTimeOverflow("game_time"))?;
        let next_overworld_time = if self.config.environment.advance_time {
            self.overworld_time
                .checked_add(1)
                .ok_or(SimulationError::EnvironmentTimeOverflow("overworld_time"))?
        } else {
            self.overworld_time
        };
        self.tick = GameTick(next_tick);
        self.game_time = next_game_time;
        self.overworld_time = next_overworld_time;
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
            environment: self.environment(),
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

    pub fn execution_report(&self) -> ExecutionReport {
        normalized_execution_report(&self.rules, &self.config, self.execution_prepared)
    }

    pub fn pending_scheduled_ticks(&self) -> usize {
        self.scheduled_ticks.len()
    }

    pub fn pending_scheduled_tick_entries(&self) -> Vec<crate::ScheduledTick> {
        self.scheduled_ticks.snapshot()
    }

    fn run_scheduled_ticks(
        &mut self,
        changes: &mut Vec<WorldEvent>,
    ) -> Result<(), SimulationError> {
        let due = self
            .scheduled_ticks
            .due_batch(self.tick, self.config.max_scheduled_ticks_per_tick);
        let (due, has_more_due) = due;
        if has_more_due {
            warn!(
                tick = self.tick.0,
                limit = self.config.max_scheduled_ticks_per_tick,
                "计划方块刻达到单刻上限"
            );
        }
        for tick in due {
            self.scheduled_ticks.start_execution(tick);
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
        let max_chained_neighbor_updates = self.config.max_chained_neighbor_updates;
        let trace_enabled = self.config.trace;
        while !stack.is_empty() {
            let multi_update = match stack.last_mut().expect("任务栈非空") {
                NeighborTask::Multi {
                    source_pos,
                    source_block,
                    skip_direction,
                    orientation,
                    next_index,
                } => {
                    while *next_index < Direction::UPDATE_ORDER.len()
                        && Some(Direction::UPDATE_ORDER[*next_index]) == *skip_direction
                    {
                        *next_index += 1;
                    }
                    if *next_index >= Direction::UPDATE_ORDER.len() {
                        stack.pop();
                        continue;
                    }
                    let direction = Direction::UPDATE_ORDER[*next_index];
                    let update = NeighborUpdate {
                        pos: source_pos.relative(direction),
                        source_pos: *source_pos,
                        source_block: *source_block,
                        orientation: *orientation,
                        moved_by_piston: false,
                    };
                    *next_index += 1;
                    if *next_index >= Direction::UPDATE_ORDER.len() {
                        stack.pop();
                    }
                    Some(update)
                }
                _ => None,
            };
            let update = if let Some(update) = multi_update {
                update
            } else {
                let task = stack.pop().expect("任务栈非空");
                match task {
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
                        for task in follow_up.into_vec().into_iter().rev() {
                            stack.push(task);
                        }
                        self.append_context_tasks(
                            phase,
                            changes,
                            &mut stack,
                            |_rules, ctx| {
                                for change in deferred_changes.into_vec() {
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
                            |rules, ctx| rules.on_deferred_task(ctx, *task),
                        )?;
                        continue;
                    }
                    NeighborTask::Single(update) => update,
                    NeighborTask::Multi { .. } => unreachable!(),
                }
            };
            count += 1;
            if count > max_chained_neighbor_updates {
                warn!(
                    tick = self.tick.0,
                    limit = max_chained_neighbor_updates,
                    "连锁邻居更新达到上限"
                );
                break;
            }

            if trace_enabled {
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
            let current_state = self.world.get_block(update.pos);
            if !self
                .rules
                .should_process_neighbor_update(update, current_state)?
            {
                continue;
            }
            self.append_context_tasks(
                phase,
                changes,
                &mut stack,
                |rules, ctx| rules.on_neighbor_update(ctx, update, current_state),
            )?;
        }
        debug!(tick = self.tick.0, count, "完成邻居更新链");
        Ok(())
    }

    fn sample_probes(&mut self) -> Vec<ProbeSample> {
        if !self.monitoring_enabled {
            return Vec::new();
        }
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
        let trace = (self.monitoring_enabled && self.config.trace).then_some(&mut self.trace);
        let events = self.config.record_events.then_some(changes);
        let mut ctx = EventContext::new(
            &mut self.world,
            self.config.mode,
            self.tick,
            self.game_time,
            self.overworld_time,
            self.config.environment.sky_light,
            phase,
            &mut self.micro_step,
            &mut self.next_sub_tick_order,
            &mut self.random_state,
            &mut self.scheduled_ticks,
            &mut self.block_events,
            &mut self.block_event_keys,
            trace,
            events,
            tasks,
            &mut self.execution_block_changes,
        );
        let callback_result = callback(&mut self.rules, &mut ctx);
        let mut passes = 0usize;
        let synchronize_result = loop {
            if !ctx.has_block_changes() {
                break Ok(());
            }
            let block_changes = ctx.take_block_changes();
            passes += 1;
            if passes > MAX_EXECUTION_SYNCHRONIZATION_PASSES {
                break Err(RulesError::Message(format!(
                    "执行器世界同步超过 {MAX_EXECUTION_SYNCHRONIZATION_PASSES} 轮"
                )));
            }
            let result = self.rules.synchronize_world(&mut ctx, &block_changes);
            ctx.recycle_block_changes(block_changes);
            if let Err(error) = result {
                break Err(error);
            }
        };
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
        callback_result?;
        synchronize_result?;
        Ok(())
    }

    fn push_trace(&mut self, phase: SimulationPhase, kind: TraceKind) {
        if !self.monitoring_enabled || !self.config.trace {
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
    #[error("环境时间溢出: {0}")]
    EnvironmentTimeOverflow(&'static str),
    #[error("执行器不可用: {0}")]
    ExecutionUnavailable(String),
}

fn validate_execution_report(
    requested_mode: ExecutionMode,
    report: &ExecutionReport,
) -> Result<(), SimulationError> {
    match requested_mode {
        ExecutionMode::Compiled if report.backend != ExecutionBackend::Compiled => {
            Err(SimulationError::ExecutionUnavailable(
                report
                    .fallback_reason
                    .clone()
                    .unwrap_or_else(|| "规则集不支持编译执行".to_owned()),
            ))
        }
        ExecutionMode::Interpreted if report.backend != ExecutionBackend::Interpreted => {
            Err(SimulationError::ExecutionUnavailable(
                "规则集没有切换到解释执行".to_owned(),
            ))
        }
        ExecutionMode::Auto => {
            if let Some(reason) = report
                .fallback_reason
                .as_deref()
                .filter(|_| report.backend == ExecutionBackend::Interpreted)
            {
                warn!(reason, "自动执行器回退到解释模式");
            }
            Ok(())
        }
        ExecutionMode::Interpreted | ExecutionMode::Compiled => Ok(()),
    }
}

fn normalized_execution_report<R: BlockRules>(
    rules: &R,
    config: &SimulationConfig,
    execution_prepared: bool,
) -> ExecutionReport {
    let mut report = rules.execution_report();
    report.requested_mode = config.execution_mode;
    if execution_prepared
        && config.execution_mode == ExecutionMode::Auto
        && report.backend == ExecutionBackend::Interpreted
        && report.fallback_reason.is_none()
    {
        report.fallback_reason = Some(
            if config.trace {
                "轨迹诊断要求解释执行"
            } else if config.mode == RedstoneMode::Experimental {
                "experimental redstone 模式要求解释执行"
            } else {
                "规则集未启用编译执行"
            }
            .to_owned(),
        );
    }
    report
}
