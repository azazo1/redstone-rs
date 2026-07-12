mod frontend;
mod graph;
mod invalidation;
mod network;
mod passes;
mod runtime;

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use redstone_core::{
    BlockChange, BlockPos, ExecutionBackend, ExecutionConfig, ExecutionMode, ExecutionReport,
    GameTick, RedstoneMode, RulesError, SparseWorld,
};
use tracing::{info, info_span, warn};

use crate::Java26Registry;

pub(crate) use self::frontend::WirePlan;
pub(crate) use self::network::NetworkInputPower;
pub(crate) use self::network::runtime::{
    OrderedWireEvent as CompiledOrderedWireEvent,
};
pub(crate) use self::runtime::{CompiledWireTransition, Synchronization};
use self::runtime::CompiledRuntime;

const AUTO_RECOMPILE_WINDOW_TICKS: u64 = 200;
const AUTO_RECOMPILE_LIMIT: usize = 11;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderedPropagation {
    Compiled,
    Interpreted,
    FellBack,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CompiledExecutor {
    report: ExecutionReport,
    runtime: Option<CompiledRuntime>,
    config: Option<ExecutionConfig>,
    permanent_fallback_reason: Option<String>,
    comparator_outputs: BTreeMap<BlockPos, u8>,
    recent_recompilations: VecDeque<GameTick>,
}

impl CompiledExecutor {
    pub(crate) fn configure(
        &mut self,
        _registry: &Java26Registry,
        _world: &SparseWorld,
        config: ExecutionConfig,
    ) -> Result<(), RulesError> {
        let diagnostic_fallback = match (config.redstone_mode, config.trace_enabled) {
            (RedstoneMode::Experimental, _) => {
                Some("experimental redstone requires interpreted execution")
            }
            (_, true) => Some("trace recording requires interpreted execution"),
            _ => None,
        };
        if let Some(reason) = diagnostic_fallback
            && config.requested_mode == ExecutionMode::Compiled
        {
            return Err(RulesError::Message(reason.to_owned()));
        }
        if let Some(reason) = &self.permanent_fallback_reason
            && config.requested_mode == ExecutionMode::Compiled
        {
            return Err(RulesError::Message(format!(
                "compiled execution is unavailable after a permanent failure: {reason}"
            )));
        }
        if self.config == Some(config) {
            return Ok(());
        }

        self.config = Some(config);
        self.runtime = None;
        self.recent_recompilations.clear();
        if config.requested_mode == ExecutionMode::Interpreted {
            self.report = ExecutionReport {
                requested_mode: config.requested_mode,
                backend: ExecutionBackend::Interpreted,
                ..ExecutionReport::default()
            };
            return Ok(());
        }
        if let Some(reason) = &self.permanent_fallback_reason {
            self.report.requested_mode = config.requested_mode;
            self.report.backend = ExecutionBackend::Interpreted;
            self.report.fallback_reason = Some(reason.clone());
            return Ok(());
        }
        if let Some(reason) = diagnostic_fallback {
            self.report = ExecutionReport {
                requested_mode: config.requested_mode,
                backend: ExecutionBackend::Interpreted,
                fallback_reason: Some(reason.to_owned()),
                ..ExecutionReport::default()
            };
            return Ok(());
        }

        self.report = ExecutionReport {
            requested_mode: config.requested_mode,
            backend: ExecutionBackend::Compiled,
            ..ExecutionReport::default()
        };
        Ok(())
    }

    pub(crate) fn prepare(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
    ) -> Result<(), RulesError> {
        if let Some(reason) = &self.permanent_fallback_reason {
            if self.report.requested_mode == ExecutionMode::Compiled {
                return Err(RulesError::Message(format!(
                    "compiled execution is unavailable after a permanent failure: {reason}"
                )));
            }
            return Ok(());
        }
        if self.runtime.is_some() || self.report.backend == ExecutionBackend::Interpreted {
            return Ok(());
        }
        let config = self.config.unwrap_or_default();

        let started = Instant::now();
        let span = info_span!("compiled_redstone_build");
        let _guard = span.enter();
        match CompiledRuntime::compile(
            registry,
            world,
            &self.comparator_outputs,
            true,
        ) {
            Ok(runtime) => {
                let compile_duration = started.elapsed();
                let node_count = runtime.node_count();
                let edge_count = runtime.edge_count();
                info!(
                    compile_ms = compile_duration.as_secs_f64() * 1000.0,
                    node_count, edge_count, "完成编译红石图"
                );
                self.runtime = Some(runtime);
                self.report = ExecutionReport {
                    requested_mode: config.requested_mode,
                    backend: ExecutionBackend::Compiled,
                    compile_duration,
                    node_count,
                    edge_count,
                    ..ExecutionReport::default()
                };
                Ok(())
            }
            Err(error) if config.requested_mode == ExecutionMode::Auto => {
                self.permanent_fallback_reason = Some(error.clone());
                self.runtime = None;
                self.report = ExecutionReport {
                    requested_mode: config.requested_mode,
                    backend: ExecutionBackend::Interpreted,
                    fallback_reason: Some(error),
                    compile_duration: started.elapsed(),
                    ..ExecutionReport::default()
                };
                Ok(())
            }
            Err(error) => Err(RulesError::Message(format!(
                "compiled redstone graph failed: {error}"
            ))),
        }
    }

    pub(crate) fn synchronize(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
        changes: &[BlockChange],
        tick: GameTick,
    ) -> Result<Synchronization, RulesError> {
        if let Some(reason) = &self.permanent_fallback_reason
            && self.report.requested_mode == ExecutionMode::Compiled
        {
            return Err(RulesError::Message(format!(
                "compiled execution is unavailable after a permanent failure: {reason}"
            )));
        }
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(Synchronization::default());
        };
        let synchronization =
            runtime.synchronize(registry, world, changes, &self.comparator_outputs);
        match synchronization {
            Ok(mut synchronization) => {
                self.refresh_report();
                if (synchronization.full_recompile || synchronization.recompiled_nodes > 0)
                    && let Some(reason) = self.auto_recompile_fallback_reason(tick)
                {
                    self.fail_runtime("compiled redstone topology changed too frequently", reason)?;
                    synchronization.fallback_to_interpreted = true;
                }
                Ok(synchronization)
            }
            Err(error) => {
                let auto = self.report.requested_mode == ExecutionMode::Auto;
                self.fail_runtime("compiled redstone synchronization failed", error)?;
                debug_assert!(auto);
                Ok(Synchronization {
                    fallback_to_interpreted: true,
                    ..Synchronization::default()
                })
            }
        }
    }

    pub(crate) fn report(&self) -> ExecutionReport {
        let mut report = self.report.clone();
        if let Some(runtime) = &self.runtime {
            report.node_count = runtime.node_count();
            report.edge_count = runtime.edge_count();
            report.compiled_updates = runtime.compiled_updates();
            report.interpreted_updates = runtime.interpreted_updates();
            report.partial_recompilations = runtime.partial_recompilations();
            report.full_recompilations = runtime.full_recompilations();
            report.dense_full_rebuilds = runtime.dense_full_rebuilds();
            report.recompiled_nodes = runtime.recompiled_nodes();
            report.recompile_duration = runtime.recompile_duration();
        }
        report
    }

    pub(crate) fn wire_plan(&mut self, pos: BlockPos) -> Option<Arc<WirePlan>> {
        let Some(runtime) = self.runtime.as_mut() else {
            self.report.interpreted_updates = self.report.interpreted_updates.saturating_add(1);
            return None;
        };
        runtime.wire_plan(pos)
    }

    #[cfg(test)]
    pub(crate) fn network_active(&self) -> bool {
        self.runtime
            .as_ref()
            .is_some_and(CompiledRuntime::network_active)
    }

    #[cfg(test)]
    pub(crate) fn network_output(&self, pos: BlockPos) -> Option<u8> {
        self.runtime.as_ref()?.network_output(pos)
    }

    pub(crate) fn batch_wire_writes(&self) -> bool {
        self.config.is_some_and(|config| !config.record_events)
    }

    pub(crate) fn propagate_ordered(
        &mut self,
        origin: BlockPos,
        events: &mut Vec<CompiledOrderedWireEvent>,
    ) -> Result<OrderedPropagation, RulesError> {
        if self.runtime.is_none()
            && self.report.requested_mode == ExecutionMode::Compiled
            && let Some(reason) = &self.permanent_fallback_reason
        {
            return Err(RulesError::Message(format!(
                "compiled execution is unavailable after a permanent failure: {reason}"
            )));
        }
        let Some(runtime) = self.runtime.as_mut() else {
            return Ok(OrderedPropagation::Interpreted);
        };
        match runtime.propagate_ordered(origin, events) {
            Ok(true) => Ok(OrderedPropagation::Compiled),
            Ok(false) => Ok(OrderedPropagation::Interpreted),
            Err(error) => {
                self.fail_runtime(
                    "compiled ordered wire propagation failed",
                    format!("{error} at {origin:?}"),
                )?;
                Ok(OrderedPropagation::FellBack)
            }
        }
    }

    pub(crate) fn fail_ordered_application(
        &mut self,
        error: RulesError,
    ) -> Result<(), RulesError> {
        self.fail_runtime(
            "compiled ordered wire application failed",
            error.to_string(),
        )
    }

    pub(crate) fn record_ordered_events(&mut self, wire: u64, boundary: u64) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.record_ordered_events(wire, boundary);
        }
    }

    pub(crate) fn resolve_wire_transition(
        &self,
        transition: self::network::runtime::WirePowerTransition,
    ) -> Result<CompiledWireTransition, RulesError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| RulesError::Message("compiled runtime is unavailable".to_owned()))?
            .resolve_wire_transition(transition)
            .map_err(RulesError::Message)
    }

    pub(crate) fn resolve_boundary(
        &self,
        target: u32,
    ) -> Result<(BlockPos, BlockPos), RulesError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| RulesError::Message("compiled runtime is unavailable".to_owned()))?
            .resolve_boundary(target)
            .map_err(RulesError::Message)
    }

    pub(crate) fn resolve_wire_observer(&self, index: u32) -> Result<BlockPos, RulesError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| RulesError::Message("compiled runtime is unavailable".to_owned()))?
            .resolve_wire_observer(index)
            .map_err(RulesError::Message)
    }

    pub(crate) fn resolve_wire_pos(&self, wire: u32) -> Result<BlockPos, RulesError> {
        self.runtime
            .as_ref()
            .ok_or_else(|| RulesError::Message("compiled runtime is unavailable".to_owned()))?
            .resolve_wire_pos(wire)
            .map_err(RulesError::Message)
    }

    pub(crate) fn wire_index(&self, pos: BlockPos) -> Option<u32> {
        self.runtime.as_ref()?.wire_index(pos)
    }

    pub(crate) fn replace_comparator_outputs(&mut self, outputs: &BTreeMap<BlockPos, u8>) {
        let previous = std::mem::replace(&mut self.comparator_outputs, outputs.clone());
        let Some(runtime) = self.runtime.as_mut() else {
            return;
        };
        for (pos, output) in outputs {
            if previous.get(pos) != Some(output) {
                runtime.update_network_source_deferred(*pos, *output);
            }
        }
        for pos in previous.keys() {
            if !outputs.contains_key(pos) {
                runtime.update_network_source_deferred(*pos, 0);
            }
        }
    }

    pub(crate) fn update_comparator_output(&mut self, pos: BlockPos, output: u8) {
        self.comparator_outputs.insert(pos, output);
        self.update_source_output(pos, output);
    }

    pub(crate) fn remove_comparator_output(&mut self, pos: BlockPos) {
        if self.comparator_outputs.remove(&pos).is_some() {
            self.update_source_output(pos, 0);
        }
    }

    pub(crate) fn update_source_output(&mut self, pos: BlockPos, output: u8) {
        if let Some(runtime) = self.runtime.as_mut() {
            runtime.update_network_source_deferred(pos, output);
        }
    }

    pub(crate) fn comparator_outputs_snapshot(&self) -> BTreeMap<BlockPos, u8> {
        self.comparator_outputs.clone()
    }

    fn refresh_report(&mut self) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        self.report.node_count = runtime.node_count();
        self.report.edge_count = runtime.edge_count();
        self.report.compiled_updates = runtime.compiled_updates();
        self.report.interpreted_updates = runtime.interpreted_updates();
        self.report.partial_recompilations = runtime.partial_recompilations();
        self.report.full_recompilations = runtime.full_recompilations();
        self.report.dense_full_rebuilds = runtime.dense_full_rebuilds();
        self.report.recompiled_nodes = runtime.recompiled_nodes();
        self.report.recompile_duration = runtime.recompile_duration();
    }

    fn auto_recompile_fallback_reason(&mut self, tick: GameTick) -> Option<String> {
        if self.report.requested_mode != ExecutionMode::Auto {
            return None;
        }
        if self
            .recent_recompilations
            .front()
            .is_some_and(|first| tick.0 < first.0)
        {
            self.recent_recompilations.clear();
        }
        while self
            .recent_recompilations
            .front()
            .is_some_and(|first| tick.0 - first.0 >= AUTO_RECOMPILE_WINDOW_TICKS)
        {
            self.recent_recompilations.pop_front();
        }
        self.recent_recompilations.push_back(tick);
        (self.recent_recompilations.len() >= AUTO_RECOMPILE_LIMIT).then(|| {
            format!(
                "{} topology recompilations within {} game ticks",
                self.recent_recompilations.len(),
                AUTO_RECOMPILE_WINDOW_TICKS
            )
        })
    }

    fn fail_runtime(
        &mut self,
        operation: &'static str,
        error: impl Into<String>,
    ) -> Result<(), RulesError> {
        let reason = format!("{operation}: {}", error.into());
        self.refresh_report();
        self.runtime = None;
        self.report.fallback_reason = Some(reason.clone());
        self.permanent_fallback_reason = Some(reason.clone());
        if self.report.requested_mode == ExecutionMode::Auto {
            self.report.backend = ExecutionBackend::Interpreted;
            warn!(%reason, "编译执行器运行失败并永久回退到解释执行");
            Ok(())
        } else {
            Err(RulesError::Message(reason))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use redstone_core::{BlockStateId, ExecutionBackend};

    use super::*;
    use crate::StateResolver;

    #[test]
    fn auto_mode_falls_back_for_experimental_redstone() {
        let registry = Java26Registry::new();
        let world = SparseWorld::new(registry.air_state());
        let mut executor = CompiledExecutor::default();

        executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Auto,
                    redstone_mode: RedstoneMode::Experimental,
                    trace_enabled: false,
                    record_events: false,
                },
            )
            .unwrap();

        assert_eq!(executor.report().backend, ExecutionBackend::Interpreted);
        assert!(executor.report().fallback_reason.is_some());
    }

    #[test]
    fn replay_event_recording_keeps_compiled_execution_available() {
        let registry = Java26Registry::new();
        let world = SparseWorld::new(registry.air_state());
        let mut executor = CompiledExecutor::default();

        executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Auto,
                    redstone_mode: RedstoneMode::Default,
                    trace_enabled: false,
                    record_events: true,
                },
            )
            .unwrap();
        executor.prepare(&registry, &world).unwrap();

        assert_eq!(executor.report().backend, ExecutionBackend::Compiled);
        assert!(executor.report().fallback_reason.is_none());
        assert!(executor.network_active());
    }

    #[test]
    fn forced_compiled_mode_rejects_experimental_redstone() {
        let registry = Java26Registry::new();
        let world = SparseWorld::new(registry.air_state());
        let mut executor = CompiledExecutor::default();

        let error = executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Compiled,
                    redstone_mode: RedstoneMode::Experimental,
                    trace_enabled: false,
                    record_events: false,
                },
            )
            .unwrap_err();

        assert!(error.to_string().contains("experimental redstone"));
        assert_eq!(executor.report(), ExecutionReport::default());
    }

    #[test]
    fn permanent_failure_survives_auto_reconfiguration() {
        let registry = Java26Registry::new();
        let world = SparseWorld::new(registry.air_state());
        let mut executor = CompiledExecutor {
            report: ExecutionReport {
                requested_mode: ExecutionMode::Auto,
                backend: ExecutionBackend::Interpreted,
                fallback_reason: Some("broken graph".to_owned()),
                compiled_updates: 41,
                interpreted_updates: 7,
                ..ExecutionReport::default()
            },
            config: Some(ExecutionConfig {
                requested_mode: ExecutionMode::Auto,
                redstone_mode: RedstoneMode::Default,
                trace_enabled: false,
                record_events: false,
            }),
            permanent_fallback_reason: Some("broken graph".to_owned()),
            ..CompiledExecutor::default()
        };

        executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Auto,
                    redstone_mode: RedstoneMode::Default,
                    trace_enabled: false,
                    record_events: true,
                },
            )
            .unwrap();

        let report = executor.report();
        assert_eq!(report.backend, ExecutionBackend::Interpreted);
        assert_eq!(report.fallback_reason.as_deref(), Some("broken graph"));
        assert_eq!(report.compiled_updates, 41);
        assert_eq!(report.interpreted_updates, 7);
        assert!(!executor.network_active());

        let forced = executor.configure(
            &registry,
            &world,
            ExecutionConfig {
                requested_mode: ExecutionMode::Compiled,
                redstone_mode: RedstoneMode::Default,
                trace_enabled: false,
                record_events: false,
            },
        );
        assert!(forced.is_err());
    }

    #[test]
    fn runtime_failure_permanently_falls_back_in_auto_and_poison_forced_compiled() {
        let registry = Java26Registry::new();
        let world = SparseWorld::new(registry.air_state());
        let mut auto = CompiledExecutor::default();
        auto.configure(
            &registry,
            &world,
            ExecutionConfig {
                requested_mode: ExecutionMode::Auto,
                redstone_mode: RedstoneMode::Default,
                trace_enabled: false,
                record_events: false,
            },
        )
        .unwrap();
        auto.prepare(&registry, &world).unwrap();

        auto.fail_ordered_application(RulesError::Message("injected failure".to_owned()))
            .unwrap();

        assert_eq!(auto.report().backend, ExecutionBackend::Interpreted);
        assert!(auto.report().fallback_reason.is_some());
        assert!(!auto.network_active());
        assert_eq!(
            auto.propagate_ordered(BlockPos::ZERO, &mut Vec::new())
                .unwrap(),
            OrderedPropagation::Interpreted
        );

        let mut forced = CompiledExecutor::default();
        forced
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Compiled,
                    redstone_mode: RedstoneMode::Default,
                    trace_enabled: false,
                    record_events: false,
                },
            )
            .unwrap();
        forced.prepare(&registry, &world).unwrap();

        assert!(
            forced
                .fail_ordered_application(RulesError::Message("injected failure".to_owned()))
                .is_err()
        );
        assert!(!forced.network_active());
        assert!(
            forced
                .propagate_ordered(BlockPos::ZERO, &mut Vec::new())
                .is_err()
        );
    }

    #[test]
    fn replacing_comparator_outputs_updates_the_prepared_network_source() {
        let mut registry = Java26Registry::new();
        let comparator = resolve_default(&mut registry, "minecraft:comparator");
        let pos = BlockPos::ZERO;
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(pos, comparator).unwrap();
        let mut executor = CompiledExecutor::default();
        executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Compiled,
                    redstone_mode: RedstoneMode::Default,
                    trace_enabled: false,
                    record_events: false,
                },
            )
            .unwrap();
        executor.prepare(&registry, &world).unwrap();
        assert_eq!(executor.network_output(pos), Some(0));

        executor.replace_comparator_outputs(&BTreeMap::from([(pos, 15)]));
        assert_eq!(executor.network_output(pos), Some(15));

        executor.replace_comparator_outputs(&BTreeMap::new());
        assert_eq!(executor.network_output(pos), Some(0));
    }

    #[test]
    fn graph_compiles_wire_nodes_and_real_inputs() {
        let mut registry = Java26Registry::new();
        let wire = resolve_default(&mut registry, "minecraft:redstone_wire");
        let block = resolve_default(&mut registry, "minecraft:redstone_block");
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(0, 0, 0), wire).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
        world.set_block(BlockPos::new(-1, 0, 0), block).unwrap();
        let runtime = CompiledRuntime::compile(&registry, &world, &BTreeMap::new(), false).unwrap();

        assert_eq!(runtime.node_count(), 3);
        assert!(runtime.edge_count() >= 2);
    }

    #[test]
    fn forced_compiled_mode_rejects_non_fixed_wire_state() {
        let mut registry = Java26Registry::new();
        let mut properties = registry
            .complete_state_properties("minecraft:redstone_wire", &BTreeMap::new())
            .unwrap();
        properties.insert("power".to_owned(), "15".to_owned());
        let wire = registry
            .resolve_state("minecraft:redstone_wire", &properties)
            .unwrap();
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, wire).unwrap();
        let mut executor = CompiledExecutor::default();
        executor
            .configure(
                &registry,
                &world,
                ExecutionConfig {
                    requested_mode: ExecutionMode::Compiled,
                    redstone_mode: RedstoneMode::Default,
                    trace_enabled: false,
                    record_events: false,
                },
            )
            .unwrap();

        let error = executor.prepare(&registry, &world).unwrap_err();
        assert!(error.to_string().contains("not a fixed point"));
    }

    #[test]
    fn auto_recompile_window_falls_back_after_more_than_ten_recompilations() {
        let mut executor = CompiledExecutor {
            report: ExecutionReport {
                requested_mode: ExecutionMode::Auto,
                backend: ExecutionBackend::Compiled,
                ..ExecutionReport::default()
            },
            ..CompiledExecutor::default()
        };

        for tick in 0..10 {
            assert!(
                executor
                    .auto_recompile_fallback_reason(GameTick(tick * 19))
                    .is_none()
            );
        }
        let reason = executor
            .auto_recompile_fallback_reason(GameTick(199))
            .unwrap();
        assert!(reason.contains("11 topology recompilations within 200 game ticks"));
    }

    #[test]
    fn auto_recompile_window_resets_and_forced_compiled_never_falls_back() {
        let mut auto = CompiledExecutor {
            report: ExecutionReport {
                requested_mode: ExecutionMode::Auto,
                backend: ExecutionBackend::Compiled,
                ..ExecutionReport::default()
            },
            ..CompiledExecutor::default()
        };
        for tick in 0..10 {
            assert!(
                auto.auto_recompile_fallback_reason(GameTick(tick))
                    .is_none()
            );
        }
        assert!(
            auto.auto_recompile_fallback_reason(GameTick(200))
                .is_none()
        );

        let mut compiled = CompiledExecutor {
            report: ExecutionReport {
                requested_mode: ExecutionMode::Compiled,
                backend: ExecutionBackend::Compiled,
                ..ExecutionReport::default()
            },
            ..CompiledExecutor::default()
        };
        for tick in 0..20 {
            assert!(
                compiled
                    .auto_recompile_fallback_reason(GameTick(tick))
                    .is_none()
            );
        }
    }

    fn resolve_default(registry: &mut Java26Registry, name: &str) -> BlockStateId {
        let properties = registry
            .complete_state_properties(name, &BTreeMap::new())
            .unwrap();
        registry.resolve_state(name, &properties).unwrap()
    }
}
