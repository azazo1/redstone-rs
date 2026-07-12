use std::collections::{VecDeque, hash_map::Entry};

use redstone_core::{BlockChange, BlockPos, Direction, SparseWorld};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::Java26Registry;

use super::frontend::{NodeKind, TopologySignature, topology_signature, wire_neighbors};
use super::graph::CompiledGraph;

const FULL_RECOMPILE_CHANGE_LIMIT: usize = 4096;
const LOCAL_RECOMPILE_PERCENT: usize = 25;
const WIRE_CLOSURE_DISTANCE: u8 = 15;

#[derive(Debug)]
pub(super) enum InvalidationPlan {
    StateOnly(Vec<(BlockPos, redstone_core::BlockStateId)>),
    Local {
        state_changes: Vec<(BlockPos, redstone_core::BlockStateId)>,
        topology_positions: FxHashSet<BlockPos>,
        network_topology_positions: FxHashSet<BlockPos>,
        positions: FxHashSet<BlockPos>,
    },
    Full {
        state_changes: Vec<(BlockPos, redstone_core::BlockStateId)>,
        network_topology_positions: FxHashSet<BlockPos>,
    },
}

#[derive(Clone, Copy, Debug, Default)]
enum SignatureCacheEntry {
    #[default]
    Unknown,
    Absent,
    Present(TopologySignature),
}

#[derive(Clone, Debug, Default)]
pub(super) struct TopologySignatureCache {
    entries: Vec<SignatureCacheEntry>,
}

impl TopologySignatureCache {
    fn get(
        &mut self,
        registry: &Java26Registry,
        state: redstone_core::BlockStateId,
    ) -> Option<TopologySignature> {
        let index = state.0 as usize;
        if self.entries.len() <= index {
            self.entries
                .resize(index + 1, SignatureCacheEntry::Unknown);
        }
        match self.entries[index] {
            SignatureCacheEntry::Absent => None,
            SignatureCacheEntry::Present(signature) => Some(signature),
            SignatureCacheEntry::Unknown => {
                let signature = topology_signature(registry, state);
                self.entries[index] = signature.map_or(
                    SignatureCacheEntry::Absent,
                    SignatureCacheEntry::Present,
                );
                signature
            }
        }
    }
}

pub(super) fn plan(
    graph: &CompiledGraph,
    registry: &Java26Registry,
    world: &SparseWorld,
    changes: &[BlockChange],
    signature_cache: &mut TopologySignatureCache,
    dense_network_active: bool,
) -> InvalidationPlan {
    let mut state_changes = Vec::new();
    let mut topology_positions = FxHashSet::default();
    let mut network_topology_positions = FxHashSet::default();
    for change in changes {
        let old = signature_cache.get(registry, change.old_state);
        let new = signature_cache.get(registry, change.new_state);
        if old == new {
            if !old.is_some_and(|signature| signature.node_kind == NodeKind::Wire) {
                state_changes.push((change.pos, change.new_state));
            }
        } else {
            topology_positions.insert(change.pos);
            // ordered boundary 已覆盖完整邻居候选, wire 形状变化只需禁用相关 fast input.
            let keeps_dense_wire_topology = old
                .is_some_and(|signature| signature.node_kind == NodeKind::Wire)
                && new.is_some_and(|signature| signature.node_kind == NodeKind::Wire);
            if !keeps_dense_wire_topology {
                network_topology_positions.insert(change.pos);
            }
        }
    }
    if topology_positions.is_empty() {
        return InvalidationPlan::StateOnly(state_changes);
    }
    if topology_positions.len() > FULL_RECOMPILE_CHANGE_LIMIT {
        return InvalidationPlan::Full {
            state_changes,
            network_topology_positions,
        };
    }
    if dense_network_active && network_topology_positions.is_empty() {
        return InvalidationPlan::Local {
            state_changes,
            topology_positions,
            network_topology_positions,
            positions: FxHashSet::default(),
        };
    }

    let mut affected = topology_positions.clone();
    for pos in &topology_positions {
        for dependent in graph.dependents(*pos) {
            affected.insert(dependent);
        }
        for direction in Direction::UPDATE_ORDER {
            let neighbor = pos.relative(direction);
            affected.insert(neighbor);
            for dependent in graph.dependents(neighbor) {
                affected.insert(dependent);
            }
        }
    }
    add_wire_closure(graph, registry, world, &topology_positions, &mut affected);

    if graph.node_count() > 0
        && affected.len().saturating_mul(100)
            > graph.node_count().saturating_mul(LOCAL_RECOMPILE_PERCENT)
    {
        InvalidationPlan::Full {
            state_changes,
            network_topology_positions,
        }
    } else {
        InvalidationPlan::Local {
            state_changes,
            topology_positions,
            network_topology_positions,
            positions: affected,
        }
    }
}

fn add_wire_closure(
    graph: &CompiledGraph,
    registry: &Java26Registry,
    world: &SparseWorld,
    dirty: &FxHashSet<BlockPos>,
    affected: &mut FxHashSet<BlockPos>,
) {
    let mut queue = VecDeque::new();
    let mut distances = FxHashMap::<BlockPos, u8>::default();
    for pos in dirty {
        for x in -2..=2 {
            for y in -2..=2 {
                for z in -2..=2 {
                    let candidate = pos.offset(x, y, z);
                    let is_wire = graph.is_wire(candidate)
                        || topology_signature(registry, world.get_block(candidate))
                            .is_some_and(|signature| signature.node_kind == NodeKind::Wire);
                    if is_wire && distances.insert(candidate, 0).is_none() {
                        queue.push_back(candidate);
                    }
                }
            }
        }
    }

    while let Some(pos) = queue.pop_front() {
        let distance = distances[&pos];
        affected.insert(pos);
        for dependent in graph.dependents(pos) {
            affected.insert(dependent);
        }
        if distance >= WIRE_CLOSURE_DISTANCE {
            continue;
        }
        for neighbor in wire_neighbors(registry, world, pos) {
            if let Entry::Vacant(entry) = distances.entry(neighbor) {
                entry.insert(distance + 1);
                queue.push_back(neighbor);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use redstone_core::{BlockChange, BlockStateId};

    use crate::StateResolver;

    use super::*;

    #[test]
    fn invalidation_threshold_constants_match_runtime_policy() {
        assert_eq!(FULL_RECOMPILE_CHANGE_LIMIT, 4096);
        assert_eq!(LOCAL_RECOMPILE_PERCENT, 25);
        assert_eq!(WIRE_CLOSURE_DISTANCE, 15);
    }

    #[test]
    fn topology_dirty_limit_switches_after_4096_positions() {
        let mut registry = Java26Registry::new();
        let old_wire = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("west", "side")],
        );
        let new_wire = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("west", "none")],
        );
        let world = SparseWorld::new(registry.air_state());
        let graph = CompiledGraph::compile(&registry, &world);

        for (count, expect_full) in [
            (FULL_RECOMPILE_CHANGE_LIMIT, false),
            (FULL_RECOMPILE_CHANGE_LIMIT + 1, true),
        ] {
            let changes = topology_changes(count, old_wire, new_wire);
            let invalidation = plan(
                &graph,
                &registry,
                &world,
                &changes,
                &mut TopologySignatureCache::default(),
                true,
            );

            match (invalidation, expect_full) {
                (
                    InvalidationPlan::Local {
                        topology_positions,
                        network_topology_positions,
                        positions,
                        ..
                    },
                    false,
                ) => {
                    assert_eq!(topology_positions.len(), count);
                    assert!(network_topology_positions.is_empty());
                    assert!(positions.is_empty());
                }
                (InvalidationPlan::Full { .. }, true) => {}
                (actual, _) => panic!("unexpected invalidation at {count} positions: {actual:?}"),
            }
        }
    }

    #[test]
    fn affected_node_ratio_switches_only_above_25_percent() {
        for (node_count, expect_full) in [(28, false), (27, true)] {
            let mut registry = Java26Registry::new();
            let source = state_with(&mut registry, "minecraft:redstone_block", &[]);
            let air = registry.air_state();
            let mut world = SparseWorld::new(air);
            for index in 0..node_count {
                world
                    .set_block(BlockPos::new(100 + index as i32 * 3, 0, 0), source)
                    .unwrap();
            }
            let graph = CompiledGraph::compile(&registry, &world);
            assert_eq!(graph.node_count(), node_count);
            world.set_block(BlockPos::ZERO, source).unwrap();
            let changes = [BlockChange {
                pos: BlockPos::ZERO,
                old_state: air,
                new_state: source,
            }];

            let invalidation = plan(
                &graph,
                &registry,
                &world,
                &changes,
                &mut TopologySignatureCache::default(),
                false,
            );

            match (invalidation, expect_full) {
                (InvalidationPlan::Local { positions, .. }, false) => {
                    assert_eq!(positions.len(), 7);
                    assert_eq!(positions.len() * 100, node_count * LOCAL_RECOMPILE_PERCENT);
                }
                (InvalidationPlan::Full { .. }, true) => {
                    assert!(7 * 100 > node_count * LOCAL_RECOMPILE_PERCENT);
                }
                (actual, _) => {
                    panic!("unexpected invalidation for {node_count} nodes: {actual:?}")
                }
            }
        }
    }

    #[test]
    fn state_only_batch_above_topology_limit_stays_state_only() {
        let mut registry = Java26Registry::new();
        let unpowered = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("power", "0")],
        );
        let powered = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("power", "1")],
        );
        let world = SparseWorld::new(registry.air_state());
        let graph = CompiledGraph::compile(&registry, &world);
        let changes = (0..=FULL_RECOMPILE_CHANGE_LIMIT)
            .map(|index| BlockChange {
                pos: BlockPos::new(index as i32, 0, 0),
                old_state: unpowered,
                new_state: powered,
            })
            .collect::<Vec<_>>();

        let InvalidationPlan::StateOnly(state_changes) =
            plan(
                &graph,
                &registry,
                &world,
                &changes,
                &mut TopologySignatureCache::default(),
                false,
            )
        else {
            panic!("state-only changes unexpectedly invalidated topology");
        };
        assert!(state_changes.is_empty());
    }

    #[test]
    fn wire_shape_changes_keep_the_dense_boundary_superset() {
        let mut registry = Java26Registry::new();
        let old_wire = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("west", "side")],
        );
        let new_wire = state_with(
            &mut registry,
            "minecraft:redstone_wire",
            &[("west", "none")],
        );
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, new_wire).unwrap();
        let graph = CompiledGraph::compile(&registry, &world);
        let changes = [BlockChange {
            pos: BlockPos::ZERO,
            old_state: old_wire,
            new_state: new_wire,
        }];

        match plan(
            &graph,
            &registry,
            &world,
            &changes,
            &mut TopologySignatureCache::default(),
            false,
        ) {
            InvalidationPlan::Local {
                topology_positions,
                network_topology_positions,
                ..
            } => {
                assert_eq!(topology_positions.len(), 1);
                assert!(network_topology_positions.is_empty());
            }
            InvalidationPlan::Full {
                network_topology_positions,
                ..
            } => assert!(network_topology_positions.is_empty()),
            InvalidationPlan::StateOnly(_) => {
                panic!("wire shape change was not classified as fallback topology")
            }
        }
    }

    #[test]
    fn repeater_orientation_change_invalidates_dense_topology() {
        let mut registry = Java26Registry::new();
        let north = state_with(
            &mut registry,
            "minecraft:repeater",
            &[("facing", "north")],
        );
        let south = state_with(
            &mut registry,
            "minecraft:repeater",
            &[("facing", "south")],
        );
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, south).unwrap();
        let graph = CompiledGraph::compile(&registry, &world);
        let changes = [BlockChange {
            pos: BlockPos::ZERO,
            old_state: north,
            new_state: south,
        }];

        match plan(
            &graph,
            &registry,
            &world,
            &changes,
            &mut TopologySignatureCache::default(),
            false,
        ) {
            InvalidationPlan::Local {
                network_topology_positions,
                ..
            }
            | InvalidationPlan::Full {
                network_topology_positions,
                ..
            } => assert_eq!(
                network_topology_positions,
                [BlockPos::ZERO].into_iter().collect()
            ),
            InvalidationPlan::StateOnly(_) => {
                panic!("repeater orientation change did not invalidate dense topology")
            }
        }
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

    fn topology_changes(
        count: usize,
        old_state: BlockStateId,
        new_state: BlockStateId,
    ) -> Vec<BlockChange> {
        (0..count)
            .map(|index| BlockChange {
                pos: BlockPos::new(index as i32, 0, 0),
                old_state,
                new_state,
            })
            .collect()
    }
}
