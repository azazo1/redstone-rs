use std::collections::VecDeque;

use redstone_core::{BlockPos, BlockStateId, Direction, SparseWorld};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;

use crate::{BlockBehavior, Java26Registry, StateDefinition};

use super::frontend::wire_neighbors;

const MAX_WIRE_ATTENUATION: u8 = 15;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct NetworkNodeId(u32);

impl NetworkNodeId {
    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct NetworkWireId(u32);

impl NetworkWireId {
    pub(super) const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) enum NetworkInputKind {
    Default,
    Side,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct NetworkInputPower {
    pub(crate) default: u8,
    pub(crate) side: u8,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct NetworkInputPort {
    source: u32,
    kind: NetworkInputKind,
    attenuation: u8,
}

impl NetworkInputPort {
    const WIRE_BIT: u32 = 1 << 31;

    const fn node(source: NetworkNodeId, kind: NetworkInputKind, attenuation: u8) -> Self {
        Self {
            source: source.0,
            kind,
            attenuation,
        }
    }

    const fn wire(source: NetworkWireId, kind: NetworkInputKind) -> Self {
        Self {
            source: Self::WIRE_BIT | source.0,
            kind,
            attenuation: 0,
        }
    }

    const fn node_id(self) -> Option<NetworkNodeId> {
        if self.source & Self::WIRE_BIT == 0 {
            Some(NetworkNodeId(self.source))
        } else {
            None
        }
    }

    const fn wire_id(self) -> Option<NetworkWireId> {
        if self.source & Self::WIRE_BIT == 0 {
            None
        } else {
            Some(NetworkWireId(self.source & !Self::WIRE_BIT))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NetworkNodeKind {
    Constant,
    Torch,
    Repeater,
    Comparator,
    Lever,
    Button,
    PressurePlate,
    Target,
    DynamicSource,
    Consumer,
}

#[derive(Clone, Debug)]
pub(super) struct NetworkNode {
    pub(super) id: NetworkNodeId,
    pub(super) pos: BlockPos,
    pub(super) state: BlockStateId,
    pub(super) kind: NetworkNodeKind,
    pub(super) initial_output: u8,
    pub(super) constant_output: Option<u8>,
    pub(super) constant_input: NetworkInputPower,
    pub(super) comparator_far_input: Option<BlockPos>,
    fast_input: bool,
    first_edge: u32,
    edge_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NetworkWire {
    pub(super) id: NetworkWireId,
    pub(super) pos: BlockPos,
    pub(super) state: BlockStateId,
    pub(super) initial_power: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NetworkEdge {
    pub(super) source: NetworkNodeId,
    pub(super) target: NetworkNodeId,
    pub(super) kind: NetworkInputKind,
    pub(super) attenuation: u8,
    pub(super) source_wire: Option<NetworkWireId>,
    pub(super) target_wire: Option<NetworkWireId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NetworkWireSource {
    pub(super) source: NetworkNodeId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NetworkWireTarget {
    pub(super) target: NetworkNodeId,
    pub(super) kind: NetworkInputKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OrderedWireTarget(u32);

impl OrderedWireTarget {
    const BOUNDARY_BIT: u32 = 1 << 31;

    const fn wire(wire: NetworkWireId) -> Self {
        Self(wire.0)
    }

    const fn boundary(index: u32) -> Self {
        Self(Self::BOUNDARY_BIT | index)
    }

    pub(super) const fn wire_id(self) -> Option<NetworkWireId> {
        if self.0 & Self::BOUNDARY_BIT == 0 {
            Some(NetworkWireId(self.0))
        } else {
            None
        }
    }

    const fn boundary_index(self) -> Option<usize> {
        if self.0 & Self::BOUNDARY_BIT == 0 {
            None
        } else {
            Some((self.0 & !Self::BOUNDARY_BIT) as usize)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OrderedBoundaryTarget {
    pub(super) pos: BlockPos,
    pub(super) source_pos: BlockPos,
    pub(super) fast_input_node: Option<NetworkNodeId>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct NetworkBuildStats {
    pub(super) wire_neighbor_scans: usize,
    pub(super) wire_neighbor_entries: usize,
    pub(super) wire_source_ports: usize,
    pub(super) wire_target_ports: usize,
    pub(super) semantic_wire_searches: usize,
    pub(super) semantic_wire_visits: usize,
}

#[derive(Clone, Debug, Default)]
struct DenseWireGraph {
    positions: Vec<BlockPos>,
    states: Vec<BlockStateId>,
    initial_power: Vec<u8>,
    position_index: FxHashMap<BlockPos, NetworkWireId>,
    dependent_offsets: Vec<u32>,
    dependents: Vec<NetworkWireId>,
    predecessor_offsets: Vec<u32>,
    predecessors: Vec<NetworkWireId>,
    source_offsets: Vec<u32>,
    sources: Vec<NetworkWireSource>,
    target_offsets: Vec<u32>,
    targets: Vec<NetworkWireTarget>,
    ordered_target_offsets: Vec<u32>,
    ordered_targets: Vec<OrderedWireTarget>,
    ordered_boundaries: Vec<OrderedBoundaryTarget>,
    observer_offsets: Vec<u32>,
    observers: Vec<BlockPos>,
    #[cfg_attr(not(test), allow(dead_code))]
    source_anchors: FxHashMap<BlockPos, SmallVec<[NetworkWireId; 4]>>,
}

impl DenseWireGraph {
    fn compile(
        registry: &Java26Registry,
        world: &SparseWorld,
        nodes: &[NetworkNode],
        node_positions: &FxHashMap<BlockPos, NetworkNodeId>,
        stats: &mut NetworkBuildStats,
    ) -> Self {
        let mut identified = world
            .iter_blocks()
            .filter_map(|(pos, state)| {
                registry
                    .state(state)
                    .is_some_and(|definition| matches!(definition.behavior, BlockBehavior::Wire))
                    .then_some((pos, state))
            })
            .collect::<Vec<_>>();
        identified.sort_unstable_by_key(|(pos, _)| *pos);

        let mut positions = Vec::with_capacity(identified.len());
        let mut states = Vec::with_capacity(identified.len());
        let mut initial_power = Vec::with_capacity(identified.len());
        let mut position_index = FxHashMap::default();
        for (index, (pos, state)) in identified.into_iter().enumerate() {
            let id = NetworkWireId(index as u32);
            let power = registry.state(state).map_or(0, |definition| definition.power);
            positions.push(pos);
            states.push(state);
            initial_power.push(power);
            position_index.insert(pos, id);
        }
        let (observer_offsets, observers) =
            compile_wire_observers(registry, world, &positions);

        let mut dependent_drafts = Vec::new();
        let mut predecessor_drafts = Vec::new();
        for (index, pos) in positions.iter().copied().enumerate() {
            stats.wire_neighbor_scans += 1;
            let target = NetworkWireId(index as u32);
            for predecessor_pos in wire_neighbors(registry, world, pos) {
                if let Some(source) = position_index.get(&predecessor_pos).copied() {
                    dependent_drafts.push((source, target));
                    predecessor_drafts.push((target, source));
                }
            }
        }
        dependent_drafts.sort_unstable();
        dependent_drafts.dedup();
        predecessor_drafts.sort_unstable();
        predecessor_drafts.dedup();
        stats.wire_neighbor_entries = dependent_drafts.len();
        let dependent_offsets = csr_offsets(
            positions.len(),
            dependent_drafts.iter().map(|(source, _)| source.index()),
        );
        let dependents = dependent_drafts
            .into_iter()
            .map(|(_, target)| target)
            .collect();
        let predecessor_offsets = csr_offsets(
            positions.len(),
            predecessor_drafts.iter().map(|(target, _)| target.index()),
        );
        let predecessors = predecessor_drafts
            .into_iter()
            .map(|(_, source)| source)
            .collect::<Vec<_>>();
        let (ordered_target_offsets, ordered_targets, ordered_boundaries) =
            compile_ordered_wire_targets(
                registry,
                world,
                &positions,
                &position_index,
                nodes,
                node_positions,
            );

        let mut source_drafts = Vec::new();
        let mut source_anchors = FxHashMap::<BlockPos, SmallVec<[NetworkWireId; 4]>>::default();
        for node in nodes {
            if !is_signal_source(node.kind) {
                continue;
            }
            let Some(state) = registry.state(node.state) else {
                continue;
            };
            for direction in Direction::UPDATE_ORDER {
                if provides_weak_output(state, direction) {
                    let wire_pos = node.pos.relative(direction.opposite());
                    if let Some(wire) = position_index.get(&wire_pos).copied() {
                        source_drafts.push(WireSourceDraft {
                            wire,
                            source: node.id,
                        });
                        source_anchors.entry(node.pos).or_default().push(wire);
                    }
                }
                if !provides_strong_output(state, direction) {
                    continue;
                }
                let bridge = node.pos.relative(direction.opposite());
                if !registry
                    .state(world.get_block(bridge))
                    .is_some_and(|definition| definition.redstone_conductor)
                {
                    continue;
                }
                for wire_direction in Direction::UPDATE_ORDER {
                    let wire_pos = bridge.relative(wire_direction.opposite());
                    if let Some(wire) = position_index.get(&wire_pos).copied() {
                        source_drafts.push(WireSourceDraft {
                            wire,
                            source: node.id,
                        });
                        source_anchors.entry(node.pos).or_default().push(wire);
                        source_anchors.entry(bridge).or_default().push(wire);
                    }
                }
            }
        }
        source_drafts.sort_unstable();
        source_drafts.dedup();
        for wires in source_anchors.values_mut() {
            wires.sort_unstable();
            wires.dedup();
        }
        stats.wire_source_ports = source_drafts.len();
        let source_offsets = csr_offsets(
            positions.len(),
            source_drafts.iter().map(|draft| draft.wire.index()),
        );
        let sources = source_drafts
            .iter()
            .map(|draft| NetworkWireSource {
                source: draft.source,
            })
            .collect();
        Self {
            positions,
            states,
            initial_power,
            position_index,
            dependent_offsets,
            dependents,
            predecessor_offsets,
            predecessors,
            source_offsets,
            sources,
            target_offsets: vec![0],
            targets: Vec::new(),
            ordered_target_offsets,
            ordered_targets,
            ordered_boundaries,
            observer_offsets,
            observers,
            source_anchors,
        }
    }

    fn install_targets(
        &mut self,
        mut drafts: Vec<WireTargetDraft>,
        stats: &mut NetworkBuildStats,
    ) {
        drafts.sort_unstable();
        drafts.dedup();
        stats.wire_target_ports = drafts.len();
        self.target_offsets = csr_offsets(
            self.positions.len(),
            drafts.iter().map(|draft| draft.wire.index()),
        );
        self.targets = drafts
            .into_iter()
            .map(|draft| NetworkWireTarget {
                target: draft.target,
                kind: draft.kind,
            })
            .collect();
    }

    fn len(&self) -> usize {
        self.positions.len()
    }

    fn id(&self, pos: BlockPos) -> Option<NetworkWireId> {
        self.position_index.get(&pos).copied()
    }

    fn wire(&self, id: NetworkWireId) -> Option<NetworkWire> {
        Some(NetworkWire {
            id,
            pos: *self.positions.get(id.index())?,
            state: *self.states.get(id.index())?,
            initial_power: *self.initial_power.get(id.index())?,
        })
    }

    fn wire_pos(&self, id: NetworkWireId) -> Option<BlockPos> {
        self.positions.get(id.index()).copied()
    }

    fn wire_pos_at(&self, index: usize) -> Option<BlockPos> {
        self.positions.get(index).copied()
    }

    fn dependents(&self, id: NetworkWireId) -> &[NetworkWireId] {
        csr_range(&self.dependent_offsets, &self.dependents, id.index())
    }

    fn predecessors(&self, id: NetworkWireId) -> &[NetworkWireId] {
        csr_range(
            &self.predecessor_offsets,
            &self.predecessors,
            id.index(),
        )
    }

    fn sources(&self, id: NetworkWireId) -> &[NetworkWireSource] {
        csr_range(&self.source_offsets, &self.sources, id.index())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn targets(&self, id: NetworkWireId) -> &[NetworkWireTarget] {
        csr_range(&self.target_offsets, &self.targets, id.index())
    }

    fn ordered_targets(&self, id: NetworkWireId) -> &[OrderedWireTarget] {
        csr_range(
            &self.ordered_target_offsets,
            &self.ordered_targets,
            id.index(),
        )
    }

    fn ordered_target_range(&self, id: NetworkWireId) -> Option<std::ops::Range<u32>> {
        let start = *self.ordered_target_offsets.get(id.index())?;
        let end = *self.ordered_target_offsets.get(id.index() + 1)?;
        Some(start..end)
    }

    fn ordered_target(&self, index: u32) -> Option<OrderedWireTarget> {
        self.ordered_targets.get(index as usize).copied()
    }

    fn ordered_boundary(&self, index: usize) -> Option<OrderedBoundaryTarget> {
        self.ordered_boundaries.get(index).copied()
    }

    fn observer_range(&self, id: NetworkWireId) -> std::ops::Range<usize> {
        let Some(offsets) = self.observer_offsets.get(id.index()..=id.index() + 1) else {
            return 0..0;
        };
        offsets[0] as usize..offsets[1] as usize
    }

    fn observer(&self, index: usize) -> Option<BlockPos> {
        self.observers.get(index).copied()
    }

    fn starts_for_change(&self, pos: BlockPos) -> SmallVec<[NetworkWireId; 16]> {
        let mut starts = SmallVec::new();
        if let Some(wire) = self.id(pos) {
            starts.push(wire);
        }
        for direction in Direction::UPDATE_ORDER {
            if let Some(wire) = self.id(pos.relative(direction)) {
                starts.push(wire);
            }
        }
        for horizontal in Direction::HORIZONTAL {
            for vertical in [Direction::Down, Direction::Up] {
                let candidate = pos.relative(vertical).relative(horizontal);
                if let Some(wire) = self.id(candidate) {
                    starts.push(wire);
                }
            }
        }
        if let Some(anchored) = self.source_anchors.get(&pos) {
            starts.extend_from_slice(anchored);
        }
        starts.sort_unstable();
        starts.dedup();
        starts
    }

    fn source_wires(&self, pos: BlockPos) -> &[NetworkWireId] {
        self.source_anchors
            .get(&pos)
            .map_or(&[], SmallVec::as_slice)
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        let len = self.positions.len();
        if self.states.len() != len
            || self.initial_power.len() != len
            || self.position_index.len() != len
        {
            return Err("dense wire column size mismatch".to_owned());
        }
        validate_csr(
            "wire dependent",
            len,
            &self.dependent_offsets,
            self.dependents.len(),
        )?;
        for target in &self.ordered_targets {
            if let Some(wire) = target.wire_id() {
                if wire.index() >= len {
                    return Err("ordered wire target is stale".to_owned());
                }
            } else if target
                .boundary_index()
                .is_none_or(|index| index >= self.ordered_boundaries.len())
            {
                return Err("ordered boundary target is stale".to_owned());
            }
        }
        validate_csr(
            "wire predecessor",
            len,
            &self.predecessor_offsets,
            self.predecessors.len(),
        )?;
        validate_csr("wire source", len, &self.source_offsets, self.sources.len())?;
        validate_csr("wire target", len, &self.target_offsets, self.targets.len())?;
        validate_csr(
            "ordered wire target",
            len,
            &self.ordered_target_offsets,
            self.ordered_targets.len(),
        )?;
        validate_csr(
            "wire observer",
            len,
            &self.observer_offsets,
            self.observers.len(),
        )?;
        if self.dependents.len() != self.predecessors.len() {
            return Err("wire direction index size mismatch".to_owned());
        }
        for (index, pos) in self.positions.iter().copied().enumerate() {
            let id = NetworkWireId(index as u32);
            if self.id(pos) != Some(id) {
                return Err(format!("dense wire position index mismatch at {pos:?}"));
            }
            let dependents = self.dependents(id);
            if dependents.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!("dense wire dependent range is not unique at {pos:?}"));
            }
            if dependents.iter().any(|dependent| dependent.index() >= len) {
                return Err(format!("dense wire dependent is stale at {pos:?}"));
            }
            let predecessors = self.predecessors(id);
            if predecessors.len() > u8::MAX as usize {
                return Err(format!(
                    "dense wire predecessor count exceeds bucket capacity at {pos:?}"
                ));
            }
            if predecessors.windows(2).any(|pair| pair[0] >= pair[1]) {
                return Err(format!("dense wire predecessor range is not unique at {pos:?}"));
            }
            if predecessors
                .iter()
                .any(|predecessor| predecessor.index() >= len)
            {
                return Err(format!("dense wire predecessor is stale at {pos:?}"));
            }
        }
        Ok(())
    }
}

fn compile_wire_observers(
    registry: &Java26Registry,
    world: &SparseWorld,
    wire_positions: &[BlockPos],
) -> (Vec<u32>, Vec<BlockPos>) {
    const OBSERVER_ORDER: [Direction; 6] = [
        Direction::West,
        Direction::East,
        Direction::North,
        Direction::South,
        Direction::Down,
        Direction::Up,
    ];

    let mut offsets = Vec::with_capacity(wire_positions.len() + 1);
    let mut observers = Vec::new();
    offsets.push(0);
    for wire_pos in wire_positions {
        for direction in OBSERVER_ORDER {
            let observer_pos = wire_pos.relative(direction);
            let observes_wire = registry
                .state(world.get_block(observer_pos))
                .is_some_and(|state| {
                    matches!(state.behavior, BlockBehavior::Observer)
                        && state.facing == Some(direction.opposite())
                });
            if observes_wire {
                observers.push(observer_pos);
            }
        }
        offsets.push(observers.len() as u32);
    }
    (offsets, observers)
}

#[derive(Clone, Debug, Default)]
pub(super) struct StaticNetwork {
    nodes: Vec<NetworkNode>,
    edges: Vec<NetworkEdge>,
    positions: FxHashMap<BlockPos, NetworkNodeId>,
    input_offsets: Vec<u32>,
    input_ports: Vec<NetworkInputPort>,
    direct_fast_target_offsets: Vec<u32>,
    direct_fast_targets: Vec<NetworkNodeId>,
    #[cfg_attr(not(test), allow(dead_code))]
    reverse_dependencies: FxHashMap<BlockPos, SmallVec<[NetworkNodeId; 4]>>,
    wires: DenseWireGraph,
    build_stats: NetworkBuildStats,
}

struct NetworkBuildParts {
    nodes: Vec<NetworkNode>,
    positions: FxHashMap<BlockPos, NetworkNodeId>,
    drafts: Vec<EdgeDraft>,
    position_dependencies: Vec<(BlockPos, NetworkNodeId)>,
    input_offsets: Vec<u32>,
    input_ports: Vec<NetworkInputPort>,
    wires: DenseWireGraph,
    build_stats: NetworkBuildStats,
}

impl StaticNetwork {
    pub(super) fn compile(registry: &Java26Registry, world: &SparseWorld) -> Self {
        let (mut nodes, positions) = identify_nodes(registry, world);
        let mut build_stats = NetworkBuildStats::default();
        let mut wires = DenseWireGraph::compile(
            registry,
            world,
            &nodes,
            &positions,
            &mut build_stats,
        );
        let (mut drafts, position_dependencies, wire_targets) = {
            let mut search = InputSearch::new(
                registry,
                world,
                &nodes,
                &positions,
                &wires,
                &mut build_stats,
            );
            for index in 0..nodes.len() {
                search.search_target(NetworkNodeId(index as u32));
            }
            search.finish()
        };
        clamp_edges(&mut drafts);
        deduplicate_edges(&mut drafts);
        fold_constants(&mut nodes, &mut drafts);
        let (input_offsets, input_ports) = compile_input_ports(nodes.len(), &drafts);
        wires.install_targets(wire_targets, &mut build_stats);
        let network = Self::finalize(NetworkBuildParts {
            nodes,
            positions,
            drafts,
            position_dependencies,
            input_offsets,
            input_ports,
            wires,
            build_stats,
        });
        debug_assert!(network.validate().is_ok());
        network
    }

    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn wire_count(&self) -> usize {
        self.wires.len()
    }

    pub(super) fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub(super) fn build_stats(&self) -> NetworkBuildStats {
        self.build_stats
    }

    pub(super) fn node_id(&self, pos: BlockPos) -> Option<NetworkNodeId> {
        self.positions.get(&pos).copied()
    }

    pub(super) fn node(&self, id: NetworkNodeId) -> Option<&NetworkNode> {
        self.nodes.get(id.index())
    }

    pub(super) fn node_has_fast_input(&self, id: NetworkNodeId) -> bool {
        self.node(id).is_some_and(|node| node.fast_input)
    }

    pub(super) fn edges_from(&self, id: NetworkNodeId) -> &[NetworkEdge] {
        let Some(node) = self.node(id) else {
            return &[];
        };
        let start = node.first_edge as usize;
        let end = start + node.edge_count as usize;
        &self.edges[start..end]
    }

    fn input_ports(&self, id: NetworkNodeId) -> &[NetworkInputPort] {
        csr_range(&self.input_offsets, &self.input_ports, id.index())
    }

    pub(super) fn direct_fast_targets(&self, id: NetworkNodeId) -> &[NetworkNodeId] {
        csr_range(
            &self.direct_fast_target_offsets,
            &self.direct_fast_targets,
            id.index(),
        )
    }

    pub(super) fn wire_id(&self, pos: BlockPos) -> Option<NetworkWireId> {
        self.wires.id(pos)
    }

    pub(super) fn wire(&self, id: NetworkWireId) -> Option<NetworkWire> {
        self.wires.wire(id)
    }

    pub(super) fn wire_pos(&self, id: NetworkWireId) -> Option<BlockPos> {
        self.wires.wire_pos(id)
    }

    pub(super) fn wire_pos_at(&self, index: usize) -> Option<BlockPos> {
        self.wires.wire_pos_at(index)
    }

    pub(super) fn wire_dependents(&self, id: NetworkWireId) -> &[NetworkWireId] {
        self.wires.dependents(id)
    }

    pub(super) fn wire_predecessors(&self, id: NetworkWireId) -> &[NetworkWireId] {
        self.wires.predecessors(id)
    }

    pub(super) fn wire_sources(&self, id: NetworkWireId) -> &[NetworkWireSource] {
        self.wires.sources(id)
    }

    pub(super) fn wire_starts_for_change(
        &self,
        pos: BlockPos,
    ) -> SmallVec<[NetworkWireId; 16]> {
        self.wires.starts_for_change(pos)
    }

    pub(super) fn source_wires(&self, pos: BlockPos) -> &[NetworkWireId] {
        self.wires.source_wires(pos)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn wire_targets(&self, id: NetworkWireId) -> &[NetworkWireTarget] {
        self.wires.targets(id)
    }

    pub(super) fn ordered_wire_targets(&self, id: NetworkWireId) -> &[OrderedWireTarget] {
        self.wires.ordered_targets(id)
    }

    pub(super) fn ordered_wire_target_range(
        &self,
        id: NetworkWireId,
    ) -> Option<std::ops::Range<u32>> {
        self.wires.ordered_target_range(id)
    }

    pub(super) fn ordered_wire_target(&self, index: u32) -> Option<OrderedWireTarget> {
        self.wires.ordered_target(index)
    }

    pub(super) fn ordered_boundary_target(
        &self,
        index: usize,
    ) -> Option<OrderedBoundaryTarget> {
        self.wires.ordered_boundary(index)
    }

    pub(super) fn wire_observer_range(&self, id: NetworkWireId) -> std::ops::Range<usize> {
        self.wires.observer_range(id)
    }

    pub(super) fn wire_observer(&self, index: usize) -> Option<BlockPos> {
        self.wires.observer(index)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn dependents(&self, pos: BlockPos) -> &[NetworkNodeId] {
        self.reverse_dependencies
            .get(&pos)
            .map_or(&[], SmallVec::as_slice)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn affected_nodes(&self, pos: BlockPos) -> Vec<NetworkNodeId> {
        let mut affected = self.dependents(pos).to_vec();
        let starts = self.wires.starts_for_change(pos);
        if starts.is_empty() {
            return affected;
        }
        let mut visited = FxHashSet::default();
        let mut queue = VecDeque::new();
        for start in starts {
            if visited.insert(start) {
                queue.push_back((start, 0u8));
            }
        }
        while let Some((wire, distance)) = queue.pop_front() {
            affected.extend(self.wire_targets(wire).iter().map(|target| target.target));
            if distance + 1 >= MAX_WIRE_ATTENUATION {
                continue;
            }
            for adjacent in self
                .wire_dependents(wire)
                .iter()
                .chain(self.wire_predecessors(wire))
            {
                if visited.insert(*adjacent) {
                    queue.push_back((*adjacent, distance + 1));
                }
            }
        }
        affected.sort_unstable();
        affected.dedup();
        affected
    }

    fn finalize(parts: NetworkBuildParts) -> Self {
        let NetworkBuildParts {
            mut nodes,
            positions,
            mut drafts,
            mut position_dependencies,
            input_offsets,
            input_ports,
            wires,
            build_stats,
        } = parts;
        drafts.sort_unstable();
        let mut edges = Vec::with_capacity(drafts.len());
        let mut cursor = 0;
        for node in &mut nodes {
            node.first_edge = edges.len() as u32;
            while let Some(draft) = drafts.get(cursor)
                && draft.source == node.id
            {
                edges.push(NetworkEdge {
                    source: draft.source,
                    target: draft.target,
                    kind: draft.kind,
                    attenuation: draft.attenuation,
                    source_wire: draft.source_wire,
                    target_wire: draft.target_wire,
                });
                cursor += 1;
            }
            node.edge_count = edges.len() as u32 - node.first_edge;
            position_dependencies.push((node.pos, node.id));
            if let Some(far) = node.comparator_far_input {
                position_dependencies.push((far, node.id));
            }
        }
        position_dependencies.sort_unstable();
        position_dependencies.dedup();
        let mut reverse_dependencies = FxHashMap::default();
        for (pos, target) in position_dependencies {
            reverse_dependencies
                .entry(pos)
                .or_insert_with(SmallVec::new)
                .push(target);
        }
        let mut direct_fast_target_drafts = edges
            .iter()
            .filter(|edge| {
                edge.source_wire.is_none()
                    && nodes
                        .get(edge.target.index())
                        .is_some_and(|node| node.fast_input)
            })
            .map(|edge| (edge.source, edge.target))
            .collect::<Vec<_>>();
        direct_fast_target_drafts.sort_unstable();
        direct_fast_target_drafts.dedup();
        let direct_fast_target_offsets = csr_offsets(
            nodes.len(),
            direct_fast_target_drafts
                .iter()
                .map(|(source, _)| source.index()),
        );
        let direct_fast_targets = direct_fast_target_drafts
            .into_iter()
            .map(|(_, target)| target)
            .collect();
        Self {
            nodes,
            edges,
            positions,
            input_offsets,
            input_ports,
            direct_fast_target_offsets,
            direct_fast_targets,
            reverse_dependencies,
            wires,
            build_stats,
        }
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.nodes.len() != self.positions.len() {
            return Err("network position index size mismatch".to_owned());
        }
        self.wires.validate()?;
        validate_csr(
            "network input",
            self.nodes.len(),
            &self.input_offsets,
            self.input_ports.len(),
        )?;
        validate_csr(
            "direct fast target",
            self.nodes.len(),
            &self.direct_fast_target_offsets,
            self.direct_fast_targets.len(),
        )?;
        for (index, node) in self.nodes.iter().enumerate() {
            if node.id.index() != index || self.node_id(node.pos) != Some(node.id) {
                return Err(format!("network node index mismatch at {:?}", node.pos));
            }
            for edge in self.edges_from(node.id) {
                if edge.source != node.id || edge.attenuation >= MAX_WIRE_ATTENUATION {
                    return Err(format!("network edge range mismatch at {:?}", node.pos));
                }
                if self.node(edge.target).is_none() {
                    return Err("network edge has a stale target".to_owned());
                }
                match (edge.source_wire, edge.target_wire) {
                    (Some(source), Some(target)) => {
                        if self.wire(source).is_none() || self.wire(target).is_none() {
                            return Err("network edge has a stale wire endpoint".to_owned());
                        }
                    }
                    (None, None) => {}
                    _ => return Err("network edge has an incomplete wire route".to_owned()),
                }
            }
        }
        Ok(())
    }
}

fn compile_input_ports(
    node_count: usize,
    edges: &[EdgeDraft],
) -> (Vec<u32>, Vec<NetworkInputPort>) {
    let mut drafts = edges
        .iter()
        .map(|edge| {
            let port = edge.target_wire.map_or_else(
                || NetworkInputPort::node(edge.source, edge.kind, edge.attenuation),
                |wire| NetworkInputPort::wire(wire, edge.kind),
            );
            (edge.target, port)
        })
        .collect::<Vec<_>>();
    drafts.sort_unstable();
    drafts.dedup();
    let offsets = csr_offsets(
        node_count,
        drafts.iter().map(|(target, _)| target.index()),
    );
    let ports = drafts.into_iter().map(|(_, port)| port).collect();
    (offsets, ports)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct EdgeDraft {
    source: NetworkNodeId,
    target: NetworkNodeId,
    kind: NetworkInputKind,
    attenuation: u8,
    source_wire: Option<NetworkWireId>,
    target_wire: Option<NetworkWireId>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WireSourceDraft {
    wire: NetworkWireId,
    source: NetworkNodeId,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct WireTargetDraft {
    wire: NetworkWireId,
    target: NetworkNodeId,
    kind: NetworkInputKind,
}

struct InputSearch<'a> {
    registry: &'a Java26Registry,
    world: &'a SparseWorld,
    nodes: &'a [NetworkNode],
    positions: &'a FxHashMap<BlockPos, NetworkNodeId>,
    wires: &'a DenseWireGraph,
    stats: &'a mut NetworkBuildStats,
    edges: Vec<EdgeDraft>,
    position_dependencies: Vec<(BlockPos, NetworkNodeId)>,
    wire_targets: Vec<WireTargetDraft>,
    visit_epoch: u32,
    visited: Vec<u32>,
    distances: Vec<u8>,
    queue: VecDeque<NetworkWireId>,
}

impl<'a> InputSearch<'a> {
    fn new(
        registry: &'a Java26Registry,
        world: &'a SparseWorld,
        nodes: &'a [NetworkNode],
        positions: &'a FxHashMap<BlockPos, NetworkNodeId>,
        wires: &'a DenseWireGraph,
        stats: &'a mut NetworkBuildStats,
    ) -> Self {
        Self {
            registry,
            world,
            nodes,
            positions,
            wires,
            stats,
            edges: Vec::new(),
            position_dependencies: Vec::new(),
            wire_targets: Vec::new(),
            visit_epoch: 0,
            visited: vec![0; wires.len()],
            distances: vec![0; wires.len()],
            queue: VecDeque::new(),
        }
    }

    fn finish(self) -> (
        Vec<EdgeDraft>,
        Vec<(BlockPos, NetworkNodeId)>,
        Vec<WireTargetDraft>,
    ) {
        (self.edges, self.position_dependencies, self.wire_targets)
    }

    fn search_target(&mut self, target: NetworkNodeId) {
        let node = &self.nodes[target.index()];
        match node.kind {
            NetworkNodeKind::Torch => {
                let Some(state) = self.registry.state(node.state) else {
                    return;
                };
                let (input, direction) = torch_input(node.pos, state);
                self.search_input(target, input, direction, NetworkInputKind::Default, false);
            }
            NetworkNodeKind::Repeater => {
                let facing = self.facing(node);
                self.search_input(
                    target,
                    node.pos.relative(facing),
                    facing,
                    NetworkInputKind::Default,
                    true,
                );
                for side in [facing.clockwise(), facing.counter_clockwise()] {
                    self.search_repeater_side(target, node.pos.relative(side), side);
                }
            }
            NetworkNodeKind::Comparator => {
                let facing = self.facing(node);
                self.search_input(
                    target,
                    node.pos.relative(facing),
                    facing,
                    NetworkInputKind::Default,
                    true,
                );
                for side in [facing.clockwise(), facing.counter_clockwise()] {
                    self.search_comparator_side(target, node.pos.relative(side), side);
                }
                if let Some(far) = node.comparator_far_input {
                    self.add_dependency(far, target);
                }
            }
            NetworkNodeKind::Consumer => {
                for direction in Direction::UPDATE_ORDER {
                    self.search_input(
                        target,
                        node.pos.relative(direction),
                        direction,
                        NetworkInputKind::Default,
                        false,
                    );
                }
            }
            NetworkNodeKind::Constant
            | NetworkNodeKind::Lever
            | NetworkNodeKind::Button
            | NetworkNodeKind::PressurePlate
            | NetworkNodeKind::Target
            | NetworkNodeKind::DynamicSource => {}
        }
    }

    fn facing(&self, node: &NetworkNode) -> Direction {
        self.registry
            .state(node.state)
            .and_then(|state| state.facing)
            .unwrap_or(Direction::North)
    }

    fn search_input(
        &mut self,
        target: NetworkNodeId,
        input_pos: BlockPos,
        direction: Direction,
        kind: NetworkInputKind,
        unconditional_wire: bool,
    ) {
        self.add_dependency(input_pos, target);
        let Some(state) = self.registry.state(self.world.get_block(input_pos)) else {
            return;
        };
        if matches!(state.behavior, BlockBehavior::Wire) {
            if unconditional_wire || wire_provides_toward(state, direction) {
                self.search_wire(target, input_pos, kind);
            }
            return;
        }
        if let Some(source) = self.signal_source(input_pos)
            && provides_weak_output(state, direction)
        {
            self.push_direct_edge(source, target, kind);
        }
        if state.redstone_conductor {
            self.search_conductor(target, input_pos, kind);
        }
    }

    fn search_repeater_side(
        &mut self,
        target: NetworkNodeId,
        source_pos: BlockPos,
        direction: Direction,
    ) {
        self.add_dependency(source_pos, target);
        let Some(state) = self.registry.state(self.world.get_block(source_pos)) else {
            return;
        };
        if !matches!(state.behavior, BlockBehavior::Repeater | BlockBehavior::Comparator)
            || !provides_weak_output(state, direction)
        {
            return;
        }
        if let Some(source) = self.signal_source(source_pos) {
            self.push_direct_edge(source, target, NetworkInputKind::Side);
        }
    }

    fn search_comparator_side(
        &mut self,
        target: NetworkNodeId,
        source_pos: BlockPos,
        direction: Direction,
    ) {
        self.add_dependency(source_pos, target);
        let Some(state) = self.registry.state(self.world.get_block(source_pos)) else {
            return;
        };
        if matches!(state.behavior, BlockBehavior::Wire) {
            self.search_wire(target, source_pos, NetworkInputKind::Side);
        } else if let Some(source) = self.signal_source(source_pos)
            && provides_weak_output(state, direction)
        {
            self.push_direct_edge(source, target, NetworkInputKind::Side);
        }
    }

    fn search_wire(
        &mut self,
        target: NetworkNodeId,
        root_pos: BlockPos,
        kind: NetworkInputKind,
    ) {
        let Some(root) = self.wires.id(root_pos) else {
            return;
        };
        self.wire_targets.push(WireTargetDraft {
            wire: root,
            target,
            kind,
        });
        self.stats.semantic_wire_searches += 1;
        self.visit_epoch = self.visit_epoch.wrapping_add(1);
        if self.visit_epoch == 0 {
            self.visited.fill(0);
            self.visit_epoch = 1;
        }
        self.queue.clear();
        self.visited[root.index()] = self.visit_epoch;
        self.distances[root.index()] = 0;
        self.queue.push_back(root);
        while let Some(wire) = self.queue.pop_front() {
            let distance = self.distances[wire.index()];
            self.stats.semantic_wire_visits += 1;
            for source in self.wires.sources(wire) {
                self.edges.push(EdgeDraft {
                    source: source.source,
                    target,
                    kind,
                    attenuation: distance,
                    source_wire: Some(wire),
                    target_wire: Some(root),
                });
            }
            if distance + 1 >= MAX_WIRE_ATTENUATION {
                continue;
            }
            for predecessor in self.wires.predecessors(wire) {
                if self.visited[predecessor.index()] == self.visit_epoch {
                    continue;
                }
                self.visited[predecessor.index()] = self.visit_epoch;
                self.distances[predecessor.index()] = distance + 1;
                self.queue.push_back(*predecessor);
            }
        }
    }

    fn search_conductor(
        &mut self,
        target: NetworkNodeId,
        conductor_pos: BlockPos,
        kind: NetworkInputKind,
    ) {
        self.add_dependency(conductor_pos, target);
        for direction in Direction::UPDATE_ORDER {
            let source_pos = conductor_pos.relative(direction);
            self.add_dependency(source_pos, target);
            let Some(state) = self.registry.state(self.world.get_block(source_pos)) else {
                continue;
            };
            if matches!(state.behavior, BlockBehavior::Wire) {
                if wire_provides_toward(state, direction) {
                    self.search_wire(target, source_pos, kind);
                }
                continue;
            }
            let Some(source) = self.signal_source(source_pos) else {
                continue;
            };
            if provides_strong_output(state, direction) {
                self.push_direct_edge(source, target, kind);
            }
        }
    }

    fn signal_source(&self, pos: BlockPos) -> Option<NetworkNodeId> {
        let id = self.positions.get(&pos).copied()?;
        is_signal_source(self.nodes[id.index()].kind).then_some(id)
    }

    fn push_direct_edge(
        &mut self,
        source: NetworkNodeId,
        target: NetworkNodeId,
        kind: NetworkInputKind,
    ) {
        self.edges.push(EdgeDraft {
            source,
            target,
            kind,
            attenuation: 0,
            source_wire: None,
            target_wire: None,
        });
    }

    fn add_dependency(&mut self, pos: BlockPos, target: NetworkNodeId) {
        self.position_dependencies.push((pos, target));
    }
}

fn identify_nodes(
    registry: &Java26Registry,
    world: &SparseWorld,
) -> (Vec<NetworkNode>, FxHashMap<BlockPos, NetworkNodeId>) {
    let mut identified = world
        .iter_blocks()
        .filter_map(|(pos, state)| {
            let definition = registry.state(state)?;
            identify_kind(&definition.behavior).map(|kind| (pos, state, kind))
        })
        .collect::<Vec<_>>();
    identified.sort_unstable_by_key(|(pos, _, _)| *pos);
    let mut positions = FxHashMap::default();
    let mut nodes = Vec::with_capacity(identified.len());
    for (index, (pos, state, kind)) in identified.into_iter().enumerate() {
        let id = NetworkNodeId(index as u32);
        let definition = registry
            .state(state)
            .expect("identified network states must remain registered");
        let comparator_far_input = if kind == NetworkNodeKind::Comparator {
            let facing = definition.facing.unwrap_or(Direction::North);
            let rear = pos.relative(facing);
            registry
                .state(world.get_block(rear))
                .is_some_and(|rear| rear.redstone_conductor)
                .then(|| rear.relative(facing))
        } else {
            None
        };
        let fast_input = matches!(
            kind,
            NetworkNodeKind::Torch | NetworkNodeKind::Repeater
        );
        nodes.push(NetworkNode {
            id,
            pos,
            state,
            kind,
            initial_output: initial_output(world, pos, definition),
            constant_output: (kind == NetworkNodeKind::Constant).then_some(15),
            constant_input: NetworkInputPower::default(),
            comparator_far_input,
            fast_input,
            first_edge: 0,
            edge_count: 0,
        });
        positions.insert(pos, id);
    }
    (nodes, positions)
}

fn identify_kind(behavior: &BlockBehavior) -> Option<NetworkNodeKind> {
    Some(match behavior {
        BlockBehavior::RedstoneBlock => NetworkNodeKind::Constant,
        BlockBehavior::Torch { .. } => NetworkNodeKind::Torch,
        BlockBehavior::Repeater => NetworkNodeKind::Repeater,
        BlockBehavior::Comparator => NetworkNodeKind::Comparator,
        BlockBehavior::Lever => NetworkNodeKind::Lever,
        BlockBehavior::Button { .. } => NetworkNodeKind::Button,
        BlockBehavior::PressurePlate { .. } => NetworkNodeKind::PressurePlate,
        BlockBehavior::Target => NetworkNodeKind::Target,
        BlockBehavior::Observer
        | BlockBehavior::TripwireHook
        | BlockBehavior::DetectorRail
        | BlockBehavior::DaylightDetector
        | BlockBehavior::Lectern
        | BlockBehavior::TrappedChest => NetworkNodeKind::DynamicSource,
        BlockBehavior::Lamp | BlockBehavior::PoweredConsumer => NetworkNodeKind::Consumer,
        _ => return None,
    })
}

fn is_signal_source(kind: NetworkNodeKind) -> bool {
    matches!(
        kind,
        NetworkNodeKind::Constant
            | NetworkNodeKind::Torch
            | NetworkNodeKind::Repeater
            | NetworkNodeKind::Comparator
            | NetworkNodeKind::Lever
            | NetworkNodeKind::Button
            | NetworkNodeKind::PressurePlate
            | NetworkNodeKind::Target
            | NetworkNodeKind::DynamicSource
    )
}

fn initial_output(world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> u8 {
    match state.behavior {
        BlockBehavior::RedstoneBlock => 15,
        BlockBehavior::Torch { .. } => u8::from(state.bool_property("lit")) * 15,
        BlockBehavior::Repeater => u8::from(state.bool_property("powered")) * 15,
        BlockBehavior::Comparator => state.power,
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
        _ => 0,
    }
}

fn provides_weak_output(state: &StateDefinition, direction: Direction) -> bool {
    match state.behavior {
        BlockBehavior::RedstoneBlock
        | BlockBehavior::Lever
        | BlockBehavior::Button { .. }
        | BlockBehavior::PressurePlate { .. }
        | BlockBehavior::Target
        | BlockBehavior::TripwireHook
        | BlockBehavior::DetectorRail
        | BlockBehavior::DaylightDetector
        | BlockBehavior::Lectern
        | BlockBehavior::TrappedChest => true,
        BlockBehavior::Torch { wall: true } => state.facing != Some(direction),
        BlockBehavior::Torch { wall: false } => direction != Direction::Up,
        BlockBehavior::Repeater | BlockBehavior::Comparator | BlockBehavior::Observer => {
            state.facing == Some(direction)
        }
        _ => false,
    }
}

fn provides_strong_output(state: &StateDefinition, direction: Direction) -> bool {
    match state.behavior {
        BlockBehavior::Torch { .. } => direction == Direction::Down,
        BlockBehavior::Lever | BlockBehavior::Button { .. } => {
            attached_direction(state) == direction
        }
        BlockBehavior::PressurePlate { .. }
        | BlockBehavior::DetectorRail
        | BlockBehavior::Lectern
        | BlockBehavior::TrappedChest => direction == Direction::Up,
        BlockBehavior::TripwireHook => state.facing == Some(direction),
        BlockBehavior::Repeater | BlockBehavior::Comparator | BlockBehavior::Observer => {
            state.facing == Some(direction)
        }
        _ => false,
    }
}

fn attached_direction(state: &StateDefinition) -> Direction {
    match state.property("face") {
        Some("ceiling") => Direction::Down,
        Some("floor") => Direction::Up,
        _ => state.facing.unwrap_or(Direction::North),
    }
}

fn torch_input(pos: BlockPos, state: &StateDefinition) -> (BlockPos, Direction) {
    match state.behavior {
        BlockBehavior::Torch { wall: true } => {
            let direction = state.facing.unwrap_or(Direction::North).opposite();
            (pos.relative(direction), direction)
        }
        _ => (pos.relative(Direction::Down), Direction::Down),
    }
}

fn wire_provides_toward(state: &StateDefinition, direction: Direction) -> bool {
    match direction {
        Direction::Down => false,
        Direction::Up => true,
        horizontal => state.property(direction_name(horizontal.opposite())) != Some("none"),
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
    }
}

fn compile_ordered_wire_targets(
    registry: &Java26Registry,
    world: &SparseWorld,
    wire_positions: &[BlockPos],
    position_index: &FxHashMap<BlockPos, NetworkWireId>,
    nodes: &[NetworkNode],
    node_positions: &FxHashMap<BlockPos, NetworkNodeId>,
) -> (
    Vec<u32>,
    Vec<OrderedWireTarget>,
    Vec<OrderedBoundaryTarget>,
) {
    let mut offsets = Vec::with_capacity(wire_positions.len() + 1);
    let mut targets = Vec::new();
    let mut boundaries = Vec::new();
    for pos in wire_positions.iter().copied() {
        offsets.push(targets.len() as u32);
        for candidate in default_wire_update_positions(pos) {
            for direction in Direction::UPDATE_ORDER {
                let target_pos = candidate.relative(direction);
                let target = if let Some(target) = position_index.get(&target_pos).copied() {
                    OrderedWireTarget::wire(target)
                } else {
                    let Some(state) = registry.state(world.get_block(target_pos)) else {
                        continue;
                    };
                    if !(state.is_rail || state.is_piston_head || state.handles_neighbor_update) {
                        continue;
                    }
                    let index = boundaries.len() as u32;
                    boundaries.push(OrderedBoundaryTarget {
                        pos: target_pos,
                        source_pos: candidate,
                        fast_input_node: node_positions
                            .get(&target_pos)
                            .copied()
                            .filter(|node| nodes[node.index()].fast_input),
                    });
                    OrderedWireTarget::boundary(index)
                };
                targets.push(target);
            }
        }
    }
    offsets.push(targets.len() as u32);
    (offsets, targets, boundaries)
}

fn default_wire_update_positions(pos: BlockPos) -> [BlockPos; 7] {
    const JAVA_DIRECTION_VALUES: [Direction; 6] = [
        Direction::Down,
        Direction::Up,
        Direction::North,
        Direction::South,
        Direction::West,
        Direction::East,
    ];
    let neighbors = JAVA_DIRECTION_VALUES.map(|direction| pos.relative(direction));
    let mut positions = [
        pos,
        neighbors[0],
        neighbors[1],
        neighbors[2],
        neighbors[3],
        neighbors[4],
        neighbors[5],
    ];
    positions.sort_by_key(|candidate| java_hash_map_bucket(*candidate));
    positions
}

fn java_hash_map_bucket(pos: BlockPos) -> u32 {
    let hash = pos
        .z
        .wrapping_mul(31)
        .wrapping_add(pos.y)
        .wrapping_mul(31)
        .wrapping_add(pos.x);
    let spread = hash ^ ((hash as u32 >> 16) as i32);
    spread as u32 & 15
}

fn csr_offsets(length: usize, owners: impl Iterator<Item = usize>) -> Vec<u32> {
    let mut offsets = vec![0u32; length + 1];
    for owner in owners {
        offsets[owner + 1] += 1;
    }
    for index in 1..offsets.len() {
        offsets[index] += offsets[index - 1];
    }
    offsets
}

fn csr_range<'a, T>(offsets: &[u32], values: &'a [T], index: usize) -> &'a [T] {
    let Some((&start, &end)) = offsets.get(index).zip(offsets.get(index + 1)) else {
        return &[];
    };
    &values[start as usize..end as usize]
}

fn validate_csr(
    name: &str,
    length: usize,
    offsets: &[u32],
    value_length: usize,
) -> Result<(), String> {
    if offsets.len() != length + 1
        || offsets.first() != Some(&0)
        || offsets.last().copied() != Some(value_length as u32)
        || offsets.windows(2).any(|pair| pair[0] > pair[1])
    {
        return Err(format!("{name} CSR range mismatch"));
    }
    Ok(())
}

fn clamp_edges(edges: &mut Vec<EdgeDraft>) {
    edges.retain(|edge| edge.attenuation < MAX_WIRE_ATTENUATION);
}

fn deduplicate_edges(edges: &mut Vec<EdgeDraft>) {
    edges.sort_unstable();
    let mut deduplicated = Vec::with_capacity(edges.len());
    for edge in edges.drain(..) {
        let duplicate = deduplicated.last().is_some_and(|previous: &EdgeDraft| {
            previous.source == edge.source
                && previous.target == edge.target
                && previous.kind == edge.kind
        });
        if !duplicate {
            deduplicated.push(edge);
        }
    }
    *edges = deduplicated;
}

fn fold_constants(nodes: &mut [NetworkNode], edges: &mut Vec<EdgeDraft>) {
    edges.retain(|edge| {
        let Some(output) = nodes[edge.source.index()].constant_output else {
            return true;
        };
        let power = output.saturating_sub(edge.attenuation);
        let input = &mut nodes[edge.target.index()].constant_input;
        match edge.kind {
            NetworkInputKind::Default => input.default = input.default.max(power),
            NetworkInputKind::Side => input.side = input.side.max(power),
        }
        false
    });
}

pub(super) mod runtime;

#[cfg(test)]
mod tests;
