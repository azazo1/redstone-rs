use std::sync::Arc;
use std::time::Instant;

use redstone_core::{BlockPos, BlockStateId, SparseWorld};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tracing::info;

use crate::Java26Registry;

use super::frontend::{
    NodeKind, TopologySignature, WirePlan, compile_wire_plan, direct_input_positions,
    topology_signature,
};
use super::passes::optimize_dependencies;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct NodeId {
    slot: u32,
    generation: u32,
}

impl NodeId {
    pub(crate) const fn new(slot: u32, generation: u32) -> Self {
        Self { slot, generation }
    }
}

#[derive(Clone, Debug)]
struct Node {
    pos: BlockPos,
    state: BlockStateId,
    signature: TopologySignature,
    wire_plan: Option<Arc<WirePlan>>,
    dependencies: SmallVec<[BlockPos; 16]>,
}

#[derive(Clone, Debug, Default)]
struct NodeSlot {
    generation: u32,
    node: Option<Node>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct CompiledGraph {
    slots: Vec<NodeSlot>,
    free_slots: Vec<u32>,
    positions: FxHashMap<BlockPos, NodeId>,
    edge_count: usize,
    reverse_dependencies: FxHashMap<BlockPos, SmallVec<[NodeId; 4]>>,
}

impl CompiledGraph {
    pub(super) fn compile(registry: &Java26Registry, world: &SparseWorld) -> Self {
        let started = Instant::now();
        let mut graph = Self::default();
        let mut signatures = FxHashMap::<BlockStateId, Option<TopologySignature>>::default();
        for (pos, state) in world.iter_blocks() {
            let signature = *signatures
                .entry(state)
                .or_insert_with(|| topology_signature(registry, state));
            if let Some(signature) = signature
                && signature.node_kind != NodeKind::Boundary
            {
                graph.insert_node(pos, state, signature);
            }
        }
        info!(
            nodes = graph.positions.len(),
            elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0,
            "完成编译节点识别"
        );
        let plans_started = Instant::now();
        let ids = graph.positions.values().copied().collect::<Vec<_>>();
        for id in ids {
            graph.rebuild_node(id, registry, world);
        }
        graph.rebuild_dependency_index();
        info!(
            nodes = graph.positions.len(),
            edges = graph.edge_count,
            elapsed_ms = plans_started.elapsed().as_secs_f64() * 1_000.0,
            "完成编译依赖索引"
        );
        graph
    }

    pub(super) fn node_count(&self) -> usize {
        self.positions.len()
    }

    pub(super) fn edge_count(&self) -> usize {
        self.edge_count
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        let occupied = self.slots.iter().filter(|slot| slot.node.is_some()).count();
        if occupied != self.positions.len() {
            return Err("compiled node position index is inconsistent".to_owned());
        }
        let expected_edges = self
            .slots
            .iter()
            .filter_map(|slot| slot.node.as_ref())
            .map(|node| node.dependencies.len())
            .sum::<usize>();
        if self.edge_count != expected_edges {
            return Err(format!(
                "compiled edge count mismatch: expected {expected_edges}, actual {}",
                self.edge_count
            ));
        }
        for (pos, id) in &self.positions {
            let Some(node) = self.node(*id) else {
                return Err(format!("compiled node id is stale at {pos:?}"));
            };
            if node.pos != *pos {
                return Err(format!("compiled node position mismatch at {pos:?}"));
            }
            let self_indexed = self
                .reverse_dependencies
                .get(pos)
                .is_some_and(|dependents| dependents.contains(id));
            if !self_indexed {
                return Err(format!("compiled self dependency is missing at {pos:?}"));
            }
            for dependency in &node.dependencies {
                let indexed = self
                    .reverse_dependencies
                    .get(dependency)
                    .is_some_and(|dependents| dependents.contains(id));
                if !indexed {
                    return Err(format!(
                        "compiled dependency index is missing {dependency:?} for {pos:?}"
                    ));
                }
            }
        }
        for (dependency, dependents) in &self.reverse_dependencies {
            if dependents.is_empty() {
                return Err(format!(
                    "compiled dependency index contains an empty entry at {dependency:?}"
                ));
            }
            if dependents.windows(2).any(|ids| ids[0] >= ids[1]) {
                return Err(format!(
                    "compiled dependency index is not sorted and unique at {dependency:?}"
                ));
            }
            for id in dependents {
                let Some(node) = self.node(*id) else {
                    return Err(format!(
                        "compiled dependency index contains a stale node at {dependency:?}"
                    ));
                };
                if node.pos != *dependency && !node.dependencies.contains(dependency) {
                    return Err(format!(
                        "compiled dependency index contains an unexpected node at {dependency:?}"
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn node_id(&self, pos: BlockPos) -> Option<NodeId> {
        self.positions.get(&pos).copied()
    }

    pub(super) fn wire_plan(&self, pos: BlockPos) -> Option<Arc<WirePlan>> {
        self.node(self.node_id(pos)?)?.wire_plan.clone()
    }

    pub(super) fn is_wire(&self, pos: BlockPos) -> bool {
        self.node_id(pos)
            .and_then(|id| self.node(id))
            .is_some_and(|node| node.signature.node_kind == NodeKind::Wire)
    }

    pub(super) fn dependents(&self, pos: BlockPos) -> SmallVec<[BlockPos; 8]> {
        let mut result = SmallVec::new();
        if let Some(ids) = self.reverse_dependencies.get(&pos) {
            for id in ids {
                if let Some(node) = self.node(*id)
                    && !result.contains(&node.pos)
                {
                    result.push(node.pos);
                }
            }
        }
        result
    }

    pub(super) fn update_state(&mut self, pos: BlockPos, state: BlockStateId) {
        let Some(id) = self.node_id(pos) else {
            return;
        };
        if let Some(node) = self.node_mut(id) {
            node.state = state;
        }
    }

    pub(super) fn recompile_positions(
        &mut self,
        registry: &Java26Registry,
        world: &SparseWorld,
        positions: &FxHashSet<BlockPos>,
    ) -> usize {
        let mut old_dependents = FxHashSet::default();
        for pos in positions {
            old_dependents.extend(self.dependents(*pos));
        }

        let mut rebuild = FxHashSet::default();
        for pos in positions {
            if let Some(old_id) = self.node_id(*pos) {
                match topology_signature(registry, world.get_block(*pos)) {
                    Some(signature) if signature.node_kind != NodeKind::Boundary => {
                        let replace = self
                            .node(old_id)
                            .is_none_or(|node| node.signature.node_kind != signature.node_kind);
                        if replace {
                            self.remove_node(old_id);
                            let id = self.insert_node(*pos, world.get_block(*pos), signature);
                            rebuild.insert(id);
                        } else if let Some(node) = self.node_mut(old_id) {
                            node.state = world.get_block(*pos);
                            node.signature = signature;
                            rebuild.insert(old_id);
                        }
                    }
                    Some(_) | None => self.remove_node(old_id),
                }
            } else if let Some(signature) = topology_signature(registry, world.get_block(*pos))
                && signature.node_kind != NodeKind::Boundary
            {
                let id = self.insert_node(*pos, world.get_block(*pos), signature);
                rebuild.insert(id);
            }
        }

        for dependent in old_dependents {
            if let Some(id) = self.node_id(dependent) {
                rebuild.insert(id);
            }
        }
        let ids = rebuild.iter().copied().collect::<Vec<_>>();
        for id in ids {
            self.remove_dependency_index(id);
            self.rebuild_node(id, registry, world);
            self.insert_dependency_index(id);
        }
        rebuild.len()
    }

    fn insert_node(
        &mut self,
        pos: BlockPos,
        state: BlockStateId,
        signature: TopologySignature,
    ) -> NodeId {
        let slot = self.free_slots.pop().unwrap_or(self.slots.len() as u32);
        if slot as usize == self.slots.len() {
            self.slots.push(NodeSlot::default());
        }
        let generation = self.slots[slot as usize].generation;
        let id = NodeId::new(slot, generation);
        self.slots[slot as usize].node = Some(Node {
            pos,
            state,
            signature,
            wire_plan: None,
            dependencies: SmallVec::new(),
        });
        self.positions.insert(pos, id);
        id
    }

    fn remove_node(&mut self, id: NodeId) {
        self.remove_dependency_index(id);
        let Some(slot) = self.slots.get_mut(id.slot as usize) else {
            return;
        };
        if slot.generation != id.generation {
            return;
        }
        if let Some(node) = slot.node.take() {
            self.positions.remove(&node.pos);
            slot.generation = slot.generation.wrapping_add(1);
            self.free_slots.push(id.slot);
        }
    }

    fn insert_dependency_index(&mut self, id: NodeId) {
        let (slots, reverse_dependencies, edge_count) = (
            &self.slots,
            &mut self.reverse_dependencies,
            &mut self.edge_count,
        );
        let Some(slot) = slots.get(id.slot as usize) else {
            return;
        };
        if slot.generation != id.generation {
            return;
        }
        let Some(node) = slot.node.as_ref() else {
            return;
        };
        Self::insert_reverse_dependency(reverse_dependencies, node.pos, id);
        for dependency in &node.dependencies {
            Self::insert_reverse_dependency(reverse_dependencies, *dependency, id);
        }
        *edge_count = edge_count
            .checked_add(node.dependencies.len())
            .expect("compiled edge count overflow");
    }

    fn remove_dependency_index(&mut self, id: NodeId) {
        let (slots, reverse_dependencies, edge_count) = (
            &self.slots,
            &mut self.reverse_dependencies,
            &mut self.edge_count,
        );
        let Some(slot) = slots.get(id.slot as usize) else {
            return;
        };
        if slot.generation != id.generation {
            return;
        }
        let Some(node) = slot.node.as_ref() else {
            return;
        };
        Self::remove_reverse_dependency(reverse_dependencies, node.pos, id);
        for dependency in &node.dependencies {
            Self::remove_reverse_dependency(reverse_dependencies, *dependency, id);
        }
        *edge_count = edge_count
            .checked_sub(node.dependencies.len())
            .expect("compiled edge count underflow");
    }

    fn insert_reverse_dependency(
        reverse_dependencies: &mut FxHashMap<BlockPos, SmallVec<[NodeId; 4]>>,
        dependency: BlockPos,
        id: NodeId,
    ) {
        let dependents = reverse_dependencies.entry(dependency).or_default();
        if let Err(index) = dependents.binary_search(&id) {
            dependents.insert(index, id);
        }
    }

    fn remove_reverse_dependency(
        reverse_dependencies: &mut FxHashMap<BlockPos, SmallVec<[NodeId; 4]>>,
        dependency: BlockPos,
        id: NodeId,
    ) {
        let remove_entry = if let Some(dependents) = reverse_dependencies.get_mut(&dependency) {
            if let Ok(index) = dependents.binary_search(&id) {
                dependents.remove(index);
            }
            dependents.is_empty()
        } else {
            false
        };
        if remove_entry {
            reverse_dependencies.remove(&dependency);
        }
    }

    fn rebuild_node(&mut self, id: NodeId, registry: &Java26Registry, world: &SparseWorld) {
        let Some(node) = self.node(id) else {
            return;
        };
        let pos = node.pos;
        let state_id = world.get_block(pos);
        let Some(state) = registry.state(state_id) else {
            return;
        };
        let mut dependencies = SmallVec::<[BlockPos; 16]>::new();
        let wire_plan = if node.signature.node_kind == NodeKind::Wire {
            let plan = compile_wire_plan(registry, world, pos);
            for input in &plan.block_inputs {
                dependencies.push(input.pos);
                dependencies.extend(input.conductor_sources.iter().map(|source| source.pos));
            }
            dependencies.extend(plan.wire_inputs.iter().copied());
            Some(Arc::new(plan))
        } else {
            dependencies.extend(
                direct_input_positions(state, pos, world, registry)
                    .into_iter()
                    .map(|(input_pos, _)| input_pos),
            );
            None
        };
        optimize_dependencies(&mut dependencies);
        if let Some(node) = self.node_mut(id) {
            node.state = state_id;
            node.dependencies = dependencies;
            node.wire_plan = wire_plan;
        }
    }

    fn rebuild_dependency_index(&mut self) {
        self.edge_count = 0;
        self.reverse_dependencies.clear();
        for slot in &self.slots {
            let Some(node) = slot.node.as_ref() else {
                continue;
            };
            let id = self.positions[&node.pos];
            self.reverse_dependencies
                .entry(node.pos)
                .or_default()
                .push(id);
            self.edge_count = self.edge_count.saturating_add(node.dependencies.len());
            for dependency in &node.dependencies {
                self.reverse_dependencies
                    .entry(*dependency)
                    .or_default()
                    .push(id);
            }
        }
        for dependents in self.reverse_dependencies.values_mut() {
            dependents.sort_unstable();
            dependents.dedup();
        }
    }

    fn node(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.slot as usize)?;
        (slot.generation == id.generation)
            .then_some(slot.node.as_ref())
            .flatten()
    }

    fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.slot as usize)?;
        (slot.generation == id.generation)
            .then_some(slot.node.as_mut())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::StateResolver;

    use super::*;

    #[test]
    fn same_kind_rebuild_updates_reverse_dependencies_and_keeps_node_id() {
        let mut registry = Java26Registry::new();
        let north_repeater = state_with(
            &mut registry,
            "minecraft:repeater",
            &[("facing", "north")],
        );
        let south_repeater = state_with(
            &mut registry,
            "minecraft:repeater",
            &[("facing", "south")],
        );
        let repeater_pos = BlockPos::ZERO;
        let old_input = BlockPos::new(0, 0, -1);
        let new_input = BlockPos::new(0, 0, 1);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(repeater_pos, north_repeater).unwrap();
        let mut graph = CompiledGraph::compile(&registry, &world);
        let old_id = graph.node_id(repeater_pos).unwrap();
        let old_edges = graph.edge_count();

        assert!(graph.dependents(old_input).contains(&repeater_pos));
        assert!(!graph.dependents(new_input).contains(&repeater_pos));

        world.set_block(repeater_pos, south_repeater).unwrap();
        let dirty = [repeater_pos].into_iter().collect();
        assert_eq!(graph.recompile_positions(&registry, &world, &dirty), 1);

        assert_eq!(graph.node_id(repeater_pos), Some(old_id));
        assert_eq!(graph.edge_count(), old_edges);
        assert!(!graph.reverse_dependencies.contains_key(&old_input));
        assert!(graph.dependents(new_input).contains(&repeater_pos));
        graph.validate().unwrap();
    }

    #[test]
    fn slot_reuse_removes_stale_generation_dependencies() {
        let mut registry = Java26Registry::new();
        let repeater = state_with(
            &mut registry,
            "minecraft:repeater",
            &[("facing", "north")],
        );
        let source = state_with(&mut registry, "minecraft:redstone_block", &[]);
        let pos = BlockPos::ZERO;
        let old_input = BlockPos::new(0, 0, -1);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(pos, repeater).unwrap();
        let mut graph = CompiledGraph::compile(&registry, &world);
        let old_id = graph.node_id(pos).unwrap();

        world.set_block(pos, source).unwrap();
        let dirty = [pos].into_iter().collect();
        assert_eq!(graph.recompile_positions(&registry, &world, &dirty), 1);

        let new_id = graph.node_id(pos).unwrap();
        assert_eq!(new_id.slot, old_id.slot);
        assert_ne!(new_id.generation, old_id.generation);
        assert!(graph.node(old_id).is_none());
        assert_eq!(graph.edge_count(), 0);
        assert!(!graph.reverse_dependencies.contains_key(&old_input));
        assert_eq!(
            graph.reverse_dependencies.get(&pos).map(SmallVec::as_slice),
            Some([new_id].as_slice())
        );
        graph.validate().unwrap();
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
}
