use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use redstone_core::{BlockChange, BlockPos, BlockStateId, SparseWorld};
use tracing::warn;

use crate::{BlockBehavior, Java26Registry};

use super::frontend::WirePlan;
use super::graph::CompiledGraph;
use super::invalidation::{self, InvalidationPlan, TopologySignatureCache};
use super::network::runtime::{NetworkRuntime, OrderedWireEvent};
use super::network::{NetworkWireId, StaticNetwork};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompiledWireTransition {
    pub(crate) wire: u32,
    pub(crate) pos: BlockPos,
    pub(crate) old_power: u8,
    pub(crate) new_power: u8,
    pub(crate) observer_start: u32,
    pub(crate) observer_count: u8,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Synchronization {
    pub(crate) full_recompile: bool,
    pub(crate) recompiled_nodes: usize,
    pub(crate) fallback_to_interpreted: bool,
}

#[derive(Clone, Debug)]
struct NetworkState {
    graph: StaticNetwork,
    runtime: NetworkRuntime,
    disabled_wires: Vec<bool>,
    disabled_wire_count: usize,
}

impl NetworkState {
    fn new(graph: StaticNetwork, runtime: NetworkRuntime) -> Self {
        let disabled_wires = vec![false; graph.wire_count()];
        Self {
            graph,
            runtime,
            disabled_wires,
            disabled_wire_count: 0,
        }
    }

    fn disable_positions<'a>(
        &mut self,
        positions: impl IntoIterator<Item = &'a BlockPos>,
    ) -> usize {
        let mut queue = VecDeque::new();
        let mut disabled = 0usize;
        for pos in positions {
            for wire in self.graph.wire_starts_for_change(*pos) {
                if disable_wire(&mut self.disabled_wires, wire) {
                    disabled += 1;
                    queue.push_back(wire);
                }
            }
        }
        while let Some(wire) = queue.pop_front() {
            for target in self.graph.ordered_wire_targets(wire) {
                let Some(target) = target.wire_id() else {
                    continue;
                };
                if disable_wire(&mut self.disabled_wires, target) {
                    disabled += 1;
                    queue.push_back(target);
                }
            }
            for target in self
                .graph
                .wire_dependents(wire)
                .iter()
                .chain(self.graph.wire_predecessors(wire))
            {
                if disable_wire(&mut self.disabled_wires, *target) {
                    disabled += 1;
                    queue.push_back(*target);
                }
            }
        }
        self.disabled_wire_count = self.disabled_wire_count.saturating_add(disabled);
        disabled
    }
}

fn disable_wire(disabled_wires: &mut [bool], wire: NetworkWireId) -> bool {
    let Some(disabled) = disabled_wires.get_mut(wire.index()) else {
        return false;
    };
    if *disabled {
        return false;
    }
    *disabled = true;
    true
}

#[derive(Clone, Debug)]
pub(super) struct CompiledRuntime {
    graph: CompiledGraph,
    network: Option<NetworkState>,
    topology_signatures: TopologySignatureCache,
    compiled_updates: u64,
    interpreted_updates: u64,
    partial_recompilations: u64,
    full_recompilations: u64,
    dense_full_rebuilds: u64,
    recompiled_nodes: u64,
    recompile_duration: Duration,
}

impl CompiledRuntime {
    pub(super) fn compile(
        registry: &Java26Registry,
        world: &SparseWorld,
        comparator_outputs: &BTreeMap<BlockPos, u8>,
        enable_network: bool,
    ) -> Result<Self, String> {
        let network = if enable_network {
            let graph = StaticNetwork::compile(registry, world);
            graph.validate()?;
            let runtime = NetworkRuntime::new(&graph, comparator_outputs);
            let mismatches = runtime.initial_wire_transitions(&graph);
            if let Some(first) = mismatches.first() {
                let pos = graph
                    .wire(first.wire)
                    .map(|wire| wire.pos)
                    .unwrap_or(BlockPos::ZERO);
                return Err(format!(
                    "initial wire state is not a fixed point at {:?}: world={}, compiled={}, mismatches={}",
                    pos,
                    first.old_power,
                    first.new_power,
                    mismatches.len()
                ));
            } else {
                Some(NetworkState::new(graph, runtime))
            }
        } else {
            None
        };
        let runtime = Self {
            graph: CompiledGraph::compile(registry, world),
            network,
            topology_signatures: TopologySignatureCache::default(),
            compiled_updates: 0,
            interpreted_updates: 0,
            partial_recompilations: 0,
            full_recompilations: 0,
            dense_full_rebuilds: 0,
            recompiled_nodes: 0,
            recompile_duration: Duration::ZERO,
        };
        runtime.graph.validate()?;
        Ok(runtime)
    }

    pub(super) fn node_count(&self) -> usize {
        self.network.as_ref().map_or_else(
            || self.graph.node_count(),
            |network| network.graph.node_count() + network.graph.wire_count(),
        )
    }

    pub(super) fn edge_count(&self) -> usize {
        self.network.as_ref().map_or_else(
            || self.graph.edge_count(),
            |network| {
                network.graph.edge_count()
                    + network.graph.build_stats().wire_neighbor_entries
            },
        )
    }

    pub(super) const fn compiled_updates(&self) -> u64 {
        self.compiled_updates
    }

    pub(super) const fn interpreted_updates(&self) -> u64 {
        self.interpreted_updates
    }

    pub(super) const fn partial_recompilations(&self) -> u64 {
        self.partial_recompilations
    }

    pub(super) const fn recompiled_nodes(&self) -> u64 {
        self.recompiled_nodes
    }

    pub(super) const fn full_recompilations(&self) -> u64 {
        self.full_recompilations
    }

    pub(super) const fn dense_full_rebuilds(&self) -> u64 {
        self.dense_full_rebuilds
    }

    pub(super) const fn recompile_duration(&self) -> Duration {
        self.recompile_duration
    }

    pub(super) fn wire_plan(&mut self, pos: BlockPos) -> Option<Arc<WirePlan>> {
        let plan = self.graph.wire_plan(pos);
        if plan.is_some() {
            self.compiled_updates = self.compiled_updates.saturating_add(1);
        } else {
            self.interpreted_updates = self.interpreted_updates.saturating_add(1);
        }
        plan
    }

    #[cfg(test)]
    pub(super) const fn network_active(&self) -> bool {
        self.network.is_some()
    }

    #[cfg(test)]
    pub(super) fn network_output(&self, pos: BlockPos) -> Option<u8> {
        let network = self.network.as_ref()?;
        let node = network.graph.node_id(pos)?;
        network.runtime.output(node)
    }

    pub(super) fn update_network_source_deferred(&mut self, pos: BlockPos, output: u8) {
        let Some(network) = self.network.as_mut() else {
            return;
        };
        let Some(source) = network.graph.node_id(pos) else {
            return;
        };
        let _ = network
            .runtime
            .update_source_deferred(&network.graph, source, output);
    }

    pub(super) fn propagate_ordered(
        &mut self,
        origin: BlockPos,
        events: &mut Vec<OrderedWireEvent>,
    ) -> Result<bool, String> {
        let Some(network) = self.network.as_mut() else {
            return Ok(false);
        };
        let Some(origin) = network.graph.wire_id(origin) else {
            return Ok(false);
        };
        let propagation = if network.disabled_wire_count == 0 {
            network
                .runtime
                .propagate_ordered_into(&network.graph, origin, events)
        } else {
            if network.disabled_wires[origin.index()] {
                return Ok(false);
            }
            network.runtime.propagate_ordered_with_disabled_into(
                &network.graph,
                origin,
                &network.disabled_wires,
                events,
            )
        };
        if let Err(error) = propagation {
            return Err(format!("ordered wire propagation failed: {error:?}"));
        }
        Ok(true)
    }

    pub(super) fn resolve_wire_transition(
        &self,
        transition: super::network::runtime::WirePowerTransition,
    ) -> Result<CompiledWireTransition, String> {
        let network = self
            .network
            .as_ref()
            .ok_or_else(|| "compiled wire event references a stale wire".to_owned())?;
        let pos = network
            .graph
            .wire_pos(transition.wire)
            .ok_or_else(|| "compiled wire event references a stale wire".to_owned())?;
        let observers = network.graph.wire_observer_range(transition.wire);
        Ok(CompiledWireTransition {
            wire: transition.wire.index() as u32,
            pos,
            old_power: transition.old_power,
            new_power: transition.new_power,
            observer_start: observers.start as u32,
            observer_count: (observers.end - observers.start) as u8,
        })
    }

    pub(super) fn resolve_wire_observer(&self, index: u32) -> Result<BlockPos, String> {
        self.network
            .as_ref()
            .and_then(|network| network.graph.wire_observer(index as usize))
            .ok_or_else(|| "compiled wire event references a stale observer".to_owned())
    }

    pub(super) fn resolve_wire_pos(&self, wire: u32) -> Result<BlockPos, String> {
        self.network
            .as_ref()
            .and_then(|network| network.graph.wire_pos_at(wire as usize))
            .ok_or_else(|| "compiled wire overlay references a stale wire".to_owned())
    }

    pub(super) fn wire_index(&self, pos: BlockPos) -> Option<u32> {
        self.network
            .as_ref()?
            .graph
            .wire_id(pos)
            .map(|wire| wire.index() as u32)
    }

    pub(super) fn resolve_boundary(&self, target: u32) -> Result<(BlockPos, BlockPos), String> {
        let boundary = self
            .network
            .as_ref()
            .and_then(|network| network.graph.ordered_boundary_target(target as usize))
            .ok_or_else(|| "compiled boundary event references a stale target".to_owned())?;
        Ok((boundary.pos, boundary.source_pos))
    }

    pub(super) fn record_ordered_events(&mut self, wire: u64, boundary: u64) {
        self.compiled_updates = self.compiled_updates.saturating_add(wire);
        self.interpreted_updates = self.interpreted_updates.saturating_add(boundary);
    }

    pub(super) fn synchronize(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
        changes: &[BlockChange],
        comparator_outputs: &BTreeMap<BlockPos, u8>,
    ) -> Result<Synchronization, String> {
        let mut result = Synchronization::default();

        let dense_network_active = self.network.is_some();
        let state_changes = match invalidation::plan(
            &self.graph,
            registry,
            world,
            changes,
            &mut self.topology_signatures,
            dense_network_active,
        ) {
            InvalidationPlan::StateOnly(state_changes) => {
                self.apply_state_changes(&state_changes);
                state_changes
            }
            InvalidationPlan::Local {
                state_changes,
                topology_positions,
                network_topology_positions,
                positions,
            } => {
                if network_topology_positions.is_empty()
                    && let Some(network) = self.network.as_mut()
                {
                    network
                        .runtime
                        .invalidate_fast_inputs(&network.graph, &topology_positions);
                }
                self.apply_state_changes(&state_changes);
                let started = Instant::now();
                let recompiled = self.graph.recompile_positions(registry, world, &positions);
                let network_recompiled = if !network_topology_positions.is_empty()
                    && self.network.is_some()
                {
                    self.rebuild_network(registry, world, comparator_outputs)?
                } else {
                    0
                };
                self.recompile_duration = self.recompile_duration.saturating_add(started.elapsed());
                #[cfg(debug_assertions)]
                self.graph.validate()?;
                result.recompiled_nodes = recompiled.saturating_add(network_recompiled);
                if result.recompiled_nodes > 0 {
                    self.partial_recompilations = self.partial_recompilations.saturating_add(1);
                }
                self.recompiled_nodes = self
                    .recompiled_nodes
                    .saturating_add(result.recompiled_nodes as u64);
                if network_recompiled > 0 {
                    warn!(
                        topology_changes = topology_positions.len(),
                        network_topology_changes = network_topology_positions.len(),
                        network_nodes = network_recompiled,
                        "rebuilt ordered network after topology changes"
                    );
                }
                state_changes
            }
            InvalidationPlan::Full {
                state_changes,
                network_topology_positions,
            } => {
                let started = Instant::now();
                self.graph = CompiledGraph::compile(registry, world);
                self.graph.validate()?;
                if network_topology_positions.is_empty()
                    && let Some(network) = self.network.as_mut()
                {
                    network.runtime.disable_all_fast_inputs();
                }
                let network_recompiled = if self.network.is_some()
                    && !network_topology_positions.is_empty()
                {
                    self.rebuild_network(registry, world, comparator_outputs)?
                } else {
                    0
                };
                self.recompile_duration = self.recompile_duration.saturating_add(started.elapsed());
                result.full_recompile = true;
                self.full_recompilations = self.full_recompilations.saturating_add(1);
                result.recompiled_nodes = self.graph.node_count().saturating_add(network_recompiled);
                self.recompiled_nodes = self
                    .recompiled_nodes
                    .saturating_add(result.recompiled_nodes as u64);
                state_changes
            }
        };
        self.synchronize_state_sources(registry, world, &state_changes, comparator_outputs);
        Ok(result)
    }

    fn apply_state_changes(&mut self, changes: &[(BlockPos, BlockStateId)]) {
        for (pos, state) in changes {
            self.graph.update_state(*pos, *state);
        }
    }

    fn rebuild_network(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
        comparator_outputs: &BTreeMap<BlockPos, u8>,
    ) -> Result<usize, String> {
        let graph = StaticNetwork::compile(registry, world);
        graph.validate()?;
        let runtime = NetworkRuntime::from_world_state(&graph, comparator_outputs);
        let recompiled = graph.node_count().saturating_add(graph.wire_count());
        self.network = Some(NetworkState::new(graph, runtime));
        self.dense_full_rebuilds = self.dense_full_rebuilds.saturating_add(1);
        Ok(recompiled)
    }

    fn synchronize_state_sources(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
        changes: &[(BlockPos, BlockStateId)],
        comparator_outputs: &BTreeMap<BlockPos, u8>,
    ) {
        for (pos, _) in changes {
            let Some(output) = source_output(
                registry,
                *pos,
                world.get_block(*pos),
                world,
                comparator_outputs,
            ) else {
                continue;
            };
            let Some(network) = self.network.as_mut() else {
                continue;
            };
            let Some(source) = network.graph.node_id(*pos) else {
                network.disable_positions(std::iter::once(pos));
                continue;
            };
            let _ = network
                .runtime
                .update_source_deferred(&network.graph, source, output);
        }
    }
}

fn source_output(
    registry: &Java26Registry,
    pos: BlockPos,
    state: BlockStateId,
    world: &SparseWorld,
    comparator_outputs: &BTreeMap<BlockPos, u8>,
) -> Option<u8> {
    let state = registry.state(state)?;
    Some(match state.behavior {
        BlockBehavior::RedstoneBlock => 15,
        BlockBehavior::Torch { .. } => u8::from(state.bool_property("lit")) * 15,
        BlockBehavior::Repeater => u8::from(state.bool_property("powered")) * 15,
        BlockBehavior::Comparator => comparator_outputs.get(&pos).copied().unwrap_or(0),
        BlockBehavior::Lever | BlockBehavior::Button { .. } => {
            u8::from(state.bool_property("powered")) * 15
        }
        BlockBehavior::PressurePlate { .. } => state
            .int_property("power")
            .map_or_else(|| u8::from(state.bool_property("powered")) * 15, |power| {
                power.clamp(0, 15) as u8
            }),
        BlockBehavior::Target => state.int_property("power").unwrap_or(0).clamp(0, 15) as u8,
        BlockBehavior::Observer => u8::from(state.bool_property("powered")) * 15,
        BlockBehavior::TripwireHook
        | BlockBehavior::DetectorRail
        | BlockBehavior::DaylightDetector => state
            .int_property("power")
            .map_or_else(|| u8::from(state.bool_property("powered")) * 15, |power| {
                power.clamp(0, 15) as u8
            }),
        BlockBehavior::Lectern => u8::from(state.bool_property("powered")) * 15,
        BlockBehavior::TrappedChest => world
            .block_entity(pos)
            .and_then(|data| data.fields.get("open_count"))
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0)
            .clamp(0, 15) as u8,
        _ => return None,
    })
}
