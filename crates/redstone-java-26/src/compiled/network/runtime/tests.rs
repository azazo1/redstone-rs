use redstone_core::BlockStateId;
use rustc_hash::FxHashMap;

use super::super::{
    DenseWireGraph, NetworkBuildParts, NetworkBuildStats, NetworkNode, NetworkWireSource,
    OrderedBoundaryTarget, csr_offsets,
};
use super::*;

#[test]
fn directed_stair_does_not_power_the_predecessor() {
    let lower = BlockPos::new(1, -1, 0);
    let upper = BlockPos::ZERO;
    let network = wire_network(&[0], &[lower, upper], &[(0, 1)], &[(1, 0)]);
    let source = NetworkNodeId(0);
    let lower = network.wire_id(lower).unwrap();
    let upper = network.wire_id(upper).unwrap();
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());

    runtime
        .update_source_deferred(&network, source, 15)
        .unwrap();
    assert_eq!(runtime.wire_power(upper), Some(0));
    let events = runtime.propagate_ordered(&network, upper).unwrap();
    assert_eq!(runtime.wire_power(lower), Some(0));
    assert_eq!(runtime.wire_power(upper), Some(15));
    assert_eq!(
        events,
        [OrderedWireEvent::Wire(WirePowerTransition {
            wire: upper,
            old_power: 0,
            new_power: 15,
        })]
    );
}

#[test]
fn ordered_branch_follows_the_precompiled_target_order() {
    let positions = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(1, 0, 0),
        BlockPos::new(0, 0, 1),
    ];
    let mut network = wire_network(&[0], &positions, &[(0, 1), (0, 2)], &[(0, 0)]);
    set_ordered_targets(
        &mut network,
        &[
            vec![
                OrderedWireTarget::wire(NetworkWireId(2)),
                OrderedWireTarget::wire(NetworkWireId(1)),
            ],
            Vec::new(),
            Vec::new(),
        ],
    );
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());
    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 15)
        .unwrap();

    let events = runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                OrderedWireEvent::Wire(transition) => Some(transition.wire),
                OrderedWireEvent::Boundary { .. } => None,
            })
            .collect::<Vec<_>>(),
        [NetworkWireId(0), NetworkWireId(2), NetworkWireId(1)]
    );
}

#[test]
fn nested_comparator_boundary_precedes_the_parent_remaining_target() {
    let positions = [BlockPos::new(0, 0, 0), BlockPos::new(1, 0, 0)];
    let nested_comparator = BlockPos::new(2, 0, 0);
    let parent_comparator = BlockPos::new(0, 0, 1);
    let mut network = wire_network(&[0], &positions, &[(0, 1)], &[(0, 0)]);
    let parent_boundary = push_ordered_boundary(&mut network, parent_comparator, positions[0]);
    let nested_boundary = push_ordered_boundary(&mut network, nested_comparator, positions[1]);
    set_ordered_targets(
        &mut network,
        &[
            vec![
                OrderedWireTarget::wire(NetworkWireId(1)),
                parent_boundary,
            ],
            vec![nested_boundary],
        ],
    );
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());
    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 15)
        .unwrap();

    let events = runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(
        events,
        [
            OrderedWireEvent::Wire(WirePowerTransition {
                wire: NetworkWireId(0),
                old_power: 0,
                new_power: 15,
            }),
            OrderedWireEvent::Wire(WirePowerTransition {
                wire: NetworkWireId(1),
                old_power: 0,
                new_power: 14,
            }),
            OrderedWireEvent::Boundary {
                target: 1,
                input: None,
            },
            OrderedWireEvent::Boundary {
                target: 0,
                input: None,
            },
        ]
    );
}

#[test]
fn source_on_and_off_propagates_in_order() {
    let positions = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(1, 0, 0),
        BlockPos::new(2, 0, 0),
    ];
    let mut network = wire_network(&[0], &positions, &[(0, 1), (1, 2)], &[(0, 0)]);
    set_ordered_targets(
        &mut network,
        &[
            vec![OrderedWireTarget::wire(NetworkWireId(1))],
            vec![OrderedWireTarget::wire(NetworkWireId(2))],
            Vec::new(),
        ],
    );
    let source = NetworkNodeId(0);
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());

    runtime
        .update_source_deferred(&network, source, 15)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [15, 14, 13]);

    runtime
        .update_source_deferred(&network, source, 0)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [0, 0, 0]);
}

#[test]
fn another_source_remains_during_ordered_propagation() {
    let positions = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(1, 0, 0),
        BlockPos::new(2, 0, 0),
    ];
    let mut network = wire_network(
        &[0, 10],
        &positions,
        &[(0, 1), (1, 2)],
        &[(0, 0), (2, 1)],
    );
    set_ordered_targets(
        &mut network,
        &[
            vec![OrderedWireTarget::wire(NetworkWireId(1))],
            vec![OrderedWireTarget::wire(NetworkWireId(2))],
            Vec::new(),
        ],
    );
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());
    assert_eq!(wire_powers(&runtime, &network), [0, 0, 10]);

    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 15)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [15, 14, 13]);

    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 0)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [0, 0, 10]);
}

#[test]
fn failed_ordered_propagation_rolls_back_predecessor_buckets() {
    let positions = [BlockPos::new(0, 0, 0), BlockPos::new(1, 0, 0)];
    let mut network = wire_network(&[0], &positions, &[(0, 1)], &[(0, 0)]);
    set_ordered_targets(
        &mut network,
        &[
            vec![OrderedWireTarget::wire(NetworkWireId(1))],
            Vec::new(),
        ],
    );
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());
    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 15)
        .unwrap();
    let mut events = Vec::new();

    assert_eq!(
        runtime.propagate_ordered_with_disabled_into(
            &network,
            NetworkWireId(0),
            &[false, true],
            &mut events,
        ),
        Err(OrderedPropagationError::DisabledWireCrossing(
            NetworkWireId(1)
        ))
    );
    assert_eq!(wire_powers(&runtime, &network), [0, 0]);
    assert_eq!(runtime.wire_predecessor_counts[1][14], 0);
    assert_eq!(runtime.wire_predecessor_masks[1], 0);

    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [15, 14]);
}

#[test]
fn predecessor_bucket_keeps_a_level_until_all_sources_leave_it() {
    let positions = [
        BlockPos::new(0, 0, 0),
        BlockPos::new(2, 0, 0),
        BlockPos::new(1, 0, 0),
    ];
    let mut network = wire_network(
        &[15, 15],
        &positions,
        &[(0, 2), (1, 2)],
        &[(0, 0), (1, 1)],
    );
    set_ordered_targets(
        &mut network,
        &[
            vec![OrderedWireTarget::wire(NetworkWireId(2))],
            vec![OrderedWireTarget::wire(NetworkWireId(2))],
            Vec::new(),
        ],
    );
    let mut runtime = NetworkRuntime::new(&network, &BTreeMap::new());
    assert_eq!(wire_powers(&runtime, &network), [15, 15, 14]);
    assert_eq!(runtime.wire_predecessor_counts[2][14], 2);

    runtime
        .update_source_deferred(&network, NetworkNodeId(0), 0)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(0))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [0, 15, 14]);
    assert_eq!(runtime.wire_predecessor_counts[2][14], 1);
    assert_ne!(runtime.wire_predecessor_masks[2] & (1 << 14), 0);

    runtime
        .update_source_deferred(&network, NetworkNodeId(1), 0)
        .unwrap();
    runtime
        .propagate_ordered(&network, NetworkWireId(1))
        .unwrap();
    assert_eq!(wire_powers(&runtime, &network), [0, 0, 0]);
    assert_eq!(runtime.wire_predecessor_counts[2][14], 0);
    assert_eq!(runtime.wire_predecessor_masks[2] & (1 << 14), 0);
}

#[test]
fn comparator_output_override_initializes_runtime_output() {
    let network = semantic_network(&[(NetworkNodeKind::Comparator, 3)]);
    let comparator_pos = network.node(NetworkNodeId(0)).unwrap().pos;
    let overrides = BTreeMap::from([(comparator_pos, 11)]);
    let runtime = NetworkRuntime::new(&network, &overrides);

    assert_eq!(runtime.output(NetworkNodeId(0)), Some(11));
}

#[test]
fn initial_wire_mismatch_is_reported_in_slot_order() {
    let positions = [BlockPos::new(0, 0, 0), BlockPos::new(1, 0, 0)];
    let mut network = wire_network(&[0], &positions, &[(0, 1)], &[]);
    network.wires.initial_power = vec![7, 5];
    let runtime = NetworkRuntime::new(&network, &BTreeMap::new());

    assert_eq!(
        runtime.initial_wire_transitions(&network),
        [
            WirePowerTransition {
                wire: NetworkWireId(0),
                old_power: 7,
                new_power: 0,
            },
            WirePowerTransition {
                wire: NetworkWireId(1),
                old_power: 5,
                new_power: 0,
            },
        ]
    );
}

fn semantic_network(definitions: &[(NetworkNodeKind, u8)]) -> StaticNetwork {
    let (nodes, positions) = nodes(definitions);
    StaticNetwork::finalize(NetworkBuildParts {
        nodes,
        positions,
        drafts: Vec::new(),
        position_dependencies: Vec::new(),
        input_offsets: vec![0; definitions.len() + 1],
        input_ports: Vec::new(),
        wires: empty_wires(),
        build_stats: NetworkBuildStats::default(),
    })
}

fn wire_network(
    outputs: &[u8],
    wire_positions: &[BlockPos],
    directed_edges: &[(usize, usize)],
    source_ports: &[(usize, usize)],
) -> StaticNetwork {
    let definitions = outputs
        .iter()
        .copied()
        .map(|output| (NetworkNodeKind::Lever, output))
        .collect::<Vec<_>>();
    let (nodes, positions) = nodes(&definitions);
    let wire_count = wire_positions.len();
    let mut position_index = FxHashMap::default();
    for (index, pos) in wire_positions.iter().copied().enumerate() {
        position_index.insert(pos, NetworkWireId(index as u32));
    }

    let mut dependents = directed_edges
        .iter()
        .map(|(source, target)| (NetworkWireId(*source as u32), NetworkWireId(*target as u32)))
        .collect::<Vec<_>>();
    dependents.sort_unstable();
    let dependent_offsets = csr_offsets(
        wire_count,
        dependents.iter().map(|(source, _)| source.index()),
    );
    let dependent_values = dependents
        .iter()
        .map(|(_, target)| *target)
        .collect::<Vec<_>>();
    let mut predecessors = dependents
        .iter()
        .map(|(source, target)| (*target, *source))
        .collect::<Vec<_>>();
    predecessors.sort_unstable();
    let predecessor_offsets = csr_offsets(
        wire_count,
        predecessors.iter().map(|(target, _)| target.index()),
    );
    let predecessor_values = predecessors
        .iter()
        .map(|(_, source)| *source)
        .collect::<Vec<_>>();

    let mut wire_sources = source_ports
        .iter()
        .map(|(wire, source)| (NetworkWireId(*wire as u32), NetworkNodeId(*source as u32)))
        .collect::<Vec<_>>();
    wire_sources.sort_unstable();
    let source_offsets = csr_offsets(
        wire_count,
        wire_sources.iter().map(|(wire, _)| wire.index()),
    );
    let source_values = wire_sources
        .iter()
        .map(|(_, source)| NetworkWireSource { source: *source })
        .collect::<Vec<_>>();
    let mut source_anchors = FxHashMap::default();
    for (wire, source) in &wire_sources {
        source_anchors
            .entry(nodes[source.index()].pos)
            .or_insert_with(smallvec::SmallVec::new)
            .push(*wire);
    }
    let wires = DenseWireGraph {
        positions: wire_positions.to_vec(),
        states: vec![BlockStateId(0); wire_count],
        initial_power: vec![0; wire_count],
        position_index,
        dependent_offsets,
        dependents: dependent_values,
        predecessor_offsets,
        predecessors: predecessor_values,
        source_offsets,
        sources: source_values,
        target_offsets: vec![0; wire_count + 1],
        targets: Vec::new(),
        ordered_target_offsets: vec![0; wire_count + 1],
        ordered_targets: Vec::new(),
        ordered_boundaries: Vec::new(),
        observer_offsets: vec![0; wire_count + 1],
        observers: Vec::new(),
        source_anchors,
    };
    StaticNetwork::finalize(NetworkBuildParts {
        nodes,
        positions,
        drafts: Vec::new(),
        position_dependencies: Vec::new(),
        input_offsets: vec![0; outputs.len() + 1],
        input_ports: Vec::new(),
        wires,
        build_stats: NetworkBuildStats::default(),
    })
}

fn set_ordered_targets(network: &mut StaticNetwork, lists: &[Vec<OrderedWireTarget>]) {
    assert_eq!(lists.len(), network.wire_count());
    let mut offsets = Vec::with_capacity(lists.len() + 1);
    let mut targets = Vec::new();
    for list in lists {
        offsets.push(targets.len() as u32);
        targets.extend_from_slice(list);
    }
    offsets.push(targets.len() as u32);
    network.wires.ordered_target_offsets = offsets;
    network.wires.ordered_targets = targets;
}

fn push_ordered_boundary(
    network: &mut StaticNetwork,
    pos: BlockPos,
    source_pos: BlockPos,
) -> OrderedWireTarget {
    let index = network.wires.ordered_boundaries.len() as u32;
    network
        .wires
        .ordered_boundaries
        .push(OrderedBoundaryTarget {
            pos,
            source_pos,
            fast_input_node: None,
        });
    OrderedWireTarget::boundary(index)
}

fn empty_wires() -> DenseWireGraph {
    DenseWireGraph {
        positions: Vec::new(),
        states: Vec::new(),
        initial_power: Vec::new(),
        position_index: FxHashMap::default(),
        dependent_offsets: vec![0],
        dependents: Vec::new(),
        predecessor_offsets: vec![0],
        predecessors: Vec::new(),
        source_offsets: vec![0],
        sources: Vec::new(),
        target_offsets: vec![0],
        targets: Vec::new(),
        ordered_target_offsets: vec![0],
        ordered_targets: Vec::new(),
        ordered_boundaries: Vec::new(),
        observer_offsets: vec![0],
        observers: Vec::new(),
        source_anchors: FxHashMap::default(),
    }
}

fn nodes(
    definitions: &[(NetworkNodeKind, u8)],
) -> (Vec<NetworkNode>, FxHashMap<BlockPos, NetworkNodeId>) {
    let mut positions = FxHashMap::default();
    let nodes = definitions
        .iter()
        .enumerate()
        .map(|(index, (kind, output))| {
            let id = NetworkNodeId(index as u32);
            let pos = BlockPos::new(100 + index as i32, 0, 0);
            positions.insert(pos, id);
            NetworkNode {
                id,
                pos,
                state: BlockStateId(index as u32),
                kind: *kind,
                initial_output: *output,
                constant_output: None,
                constant_input: NetworkInputPower::default(),
                comparator_far_input: None,
                fast_input: false,
                first_edge: 0,
                edge_count: 0,
            }
        })
        .collect();
    (nodes, positions)
}

fn wire_powers(runtime: &NetworkRuntime, network: &StaticNetwork) -> Vec<u8> {
    (0..network.wire_count())
        .map(|index| runtime.wire_power(NetworkWireId(index as u32)).unwrap())
        .collect()
}
