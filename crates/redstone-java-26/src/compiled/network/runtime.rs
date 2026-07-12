use std::collections::{BTreeMap, VecDeque};

use redstone_core::BlockPos;

use super::{
    NetworkInputKind, NetworkInputPower, NetworkNodeId, NetworkNodeKind, NetworkWireId,
    OrderedWireTarget, StaticNetwork,
};

const ORDERED_PROPAGATION_LIMIT: usize = 4_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WirePowerTransition {
    pub(in crate::compiled) wire: NetworkWireId,
    pub(in crate::compiled) old_power: u8,
    pub(crate) new_power: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderedWireEvent {
    Wire(WirePowerTransition),
    Boundary {
        target: u32,
        input: Option<NetworkInputPower>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::compiled) enum OrderedPropagationError {
    InvalidWire(NetworkWireId),
    InvalidOrderedTarget(u32),
    InvalidBoundary(usize),
    DisabledWireCrossing(NetworkWireId),
    IterationLimitExceeded { limit: usize },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OrderedTargetFrame {
    next: u32,
    end: u32,
}

#[derive(Clone, Debug)]
pub(in crate::compiled) struct NetworkRuntime {
    outputs: Vec<u8>,
    wire_powers: Vec<u8>,
    direct_wire_powers: Vec<u8>,
    wire_predecessor_counts: Vec<[u8; 16]>,
    wire_predecessor_masks: Vec<u16>,
    fast_inputs: Vec<NetworkInputPower>,
    fast_input_enabled: Vec<bool>,
    fast_input_emitted_epochs: Vec<u32>,
    fast_input_dirty_epochs: Vec<u32>,
    fast_input_epoch: u32,
    wire_inputs_dirty: Vec<bool>,
    queue: VecDeque<NetworkWireId>,
    ordered_stack: Vec<OrderedTargetFrame>,
}

impl NetworkRuntime {
    pub(in crate::compiled) fn new(
        network: &StaticNetwork,
        comparator_outputs: &BTreeMap<BlockPos, u8>,
    ) -> Self {
        let mut runtime = Self::from_world_state(network, comparator_outputs);
        runtime.rebuild_all_wires(network);
        runtime.rebuild_predecessor_buckets(network);
        runtime.refresh_fast_inputs(network);
        runtime
    }

    pub(in crate::compiled) fn from_world_state(
        network: &StaticNetwork,
        comparator_outputs: &BTreeMap<BlockPos, u8>,
    ) -> Self {
        let outputs = network
            .nodes
            .iter()
            .map(|node| {
                if node.kind == NetworkNodeKind::Comparator {
                    comparator_outputs
                        .get(&node.pos)
                        .copied()
                        .unwrap_or_else(|| node.constant_output.unwrap_or(node.initial_output))
                } else {
                    node.constant_output.unwrap_or(node.initial_output)
                }
                .min(15)
            })
            .collect::<Vec<_>>();
        let mut runtime = Self {
            outputs,
            wire_powers: (0..network.wire_count())
                .map(|index| {
                    network
                        .wire(NetworkWireId(index as u32))
                        .map_or(0, |wire| wire.initial_power)
                })
                .collect(),
            direct_wire_powers: vec![0; network.wire_count()],
            wire_predecessor_counts: vec![[0; 16]; network.wire_count()],
            wire_predecessor_masks: vec![0; network.wire_count()],
            fast_inputs: vec![NetworkInputPower::default(); network.node_count()],
            fast_input_enabled: (0..network.node_count())
                .map(|index| network.node_has_fast_input(NetworkNodeId(index as u32)))
                .collect(),
            fast_input_emitted_epochs: vec![0; network.node_count()],
            fast_input_dirty_epochs: vec![0; network.node_count()],
            fast_input_epoch: 0,
            wire_inputs_dirty: vec![false; network.wire_count()],
            queue: VecDeque::new(),
            ordered_stack: Vec::new(),
        };
        runtime.rebuild_direct_wire_powers(network);
        runtime.rebuild_predecessor_buckets(network);
        runtime.refresh_fast_inputs(network);
        runtime
    }

    #[cfg(test)]
    pub(in crate::compiled) fn output(&self, node: NetworkNodeId) -> Option<u8> {
        self.outputs.get(node.index()).copied()
    }

    pub(super) fn wire_power(&self, wire: NetworkWireId) -> Option<u8> {
        self.wire_powers.get(wire.index()).copied()
    }

    pub(in crate::compiled) fn initial_wire_transitions(
        &self,
        network: &StaticNetwork,
    ) -> Vec<WirePowerTransition> {
        (0..network.wire_count())
            .filter_map(|index| {
                let wire = NetworkWireId(index as u32);
                let initial = network.wire(wire)?;
                let new_power = self.wire_power(wire)?;
                (initial.initial_power != new_power).then_some(WirePowerTransition {
                    wire,
                    old_power: initial.initial_power,
                    new_power,
                })
            })
            .collect()
    }

    pub(in crate::compiled) fn update_source_deferred(
        &mut self,
        network: &StaticNetwork,
        source: NetworkNodeId,
        new_output: u8,
    ) -> Option<()> {
        let old_output = *self.outputs.get(source.index())?;
        let new_output = new_output.min(15);
        if old_output == new_output {
            return Some(());
        }
        self.outputs[source.index()] = new_output;
        let source_pos = network.node(source)?.pos;
        for wire in network.source_wires(source_pos) {
            self.direct_wire_powers[wire.index()] =
                self.scan_direct_wire_power(network, *wire);
        }
        for target in network.direct_fast_targets(source) {
            let index = target.index();
            let next = self.node_input_power(network, *target);
            if self.fast_inputs[index] != next {
                self.fast_inputs[index] = next;
            }
        }
        Some(())
    }

    #[cfg(test)]
    pub(in crate::compiled) fn propagate_ordered(
        &mut self,
        network: &StaticNetwork,
        origin: NetworkWireId,
    ) -> Result<Vec<OrderedWireEvent>, OrderedPropagationError> {
        let mut events = Vec::new();
        self.propagate_ordered_into(network, origin, &mut events)?;
        Ok(events)
    }

    pub(in crate::compiled) fn propagate_ordered_into(
        &mut self,
        network: &StaticNetwork,
        origin: NetworkWireId,
        events: &mut Vec<OrderedWireEvent>,
    ) -> Result<(), OrderedPropagationError> {
        self.propagate_ordered_inner::<false>(network, origin, &[], events)
    }

    pub(in crate::compiled) fn propagate_ordered_with_disabled_into(
        &mut self,
        network: &StaticNetwork,
        origin: NetworkWireId,
        disabled_wires: &[bool],
        events: &mut Vec<OrderedWireEvent>,
    ) -> Result<(), OrderedPropagationError> {
        self.propagate_ordered_inner::<true>(network, origin, disabled_wires, events)
    }

    fn propagate_ordered_inner<const CHECK_DISABLED: bool>(
        &mut self,
        network: &StaticNetwork,
        origin: NetworkWireId,
        disabled_wires: &[bool],
        events: &mut Vec<OrderedWireEvent>,
    ) -> Result<(), OrderedPropagationError> {
        events.clear();
        self.ordered_stack.clear();
        self.fast_input_epoch = self.fast_input_epoch.wrapping_add(1);
        if self.fast_input_epoch == 0 {
            self.fast_input_emitted_epochs.fill(0);
            self.fast_input_dirty_epochs.fill(0);
            self.fast_input_epoch = 1;
        }
        if self.wire_power(origin).is_none() || network.wire(origin).is_none() {
            return Err(OrderedPropagationError::InvalidWire(origin));
        }
        self.wire_inputs_dirty[origin.index()] = true;
        let mut target = OrderedWireTarget::wire(origin);
        let mut current_frame = OrderedTargetFrame { next: 0, end: 0 };
        let mut iterations = 0usize;
        loop {
            iterations += 1;
            if iterations > ORDERED_PROPAGATION_LIMIT {
                self.rollback_ordered(network, events);
                return Err(OrderedPropagationError::IterationLimitExceeded {
                    limit: ORDERED_PROPAGATION_LIMIT,
                });
            }
            if let Some(wire) = target.wire_id() {
                if CHECK_DISABLED
                    && disabled_wires.get(wire.index()).copied().unwrap_or(false)
                {
                    self.rollback_ordered(network, events);
                    return Err(OrderedPropagationError::DisabledWireCrossing(wire));
                }
                let Some(dirty) = self.wire_inputs_dirty.get_mut(wire.index()) else {
                    self.rollback_ordered(network, events);
                    return Err(OrderedPropagationError::InvalidWire(wire));
                };
                if *dirty {
                    *dirty = false;
                    let Some(old_power) = self.wire_power(wire) else {
                        self.rollback_ordered(network, events);
                        return Err(OrderedPropagationError::InvalidWire(wire));
                    };
                    let new_power = self.wire_target_power(network, wire);
                    if old_power != new_power {
                        self.wire_powers[wire.index()] = new_power;
                        let old_contribution = old_power.saturating_sub(1);
                        let new_contribution = new_power.saturating_sub(1);
                        let dependents = network.wire_dependents(wire);
                        for (index, dependent) in dependents.iter().enumerate() {
                            let Some(dirty) = self.wire_inputs_dirty.get_mut(dependent.index()) else {
                                if old_contribution != new_contribution {
                                    for updated in &dependents[..index] {
                                        self.update_predecessor_bucket(
                                            *updated,
                                            new_contribution,
                                            old_contribution,
                                        );
                                    }
                                }
                                self.wire_powers[wire.index()] = old_power;
                                self.rollback_ordered(network, events);
                                return Err(OrderedPropagationError::InvalidWire(*dependent));
                            };
                            *dirty = true;
                            if old_contribution != new_contribution {
                                self.update_predecessor_bucket(
                                    *dependent,
                                    old_contribution,
                                    new_contribution,
                                );
                            }
                        }
                        events.push(OrderedWireEvent::Wire(WirePowerTransition {
                            wire,
                            old_power,
                            new_power,
                        }));
                        self.update_fast_wire_inputs(network, wire, new_power);
                        let Some(range) = network.ordered_wire_target_range(wire) else {
                            self.rollback_ordered(network, events);
                            return Err(OrderedPropagationError::InvalidWire(wire));
                        };
                        if !range.is_empty() {
                            if current_frame.next != current_frame.end {
                                self.ordered_stack.push(current_frame);
                            }
                            current_frame = OrderedTargetFrame {
                                next: range.start,
                                end: range.end,
                            };
                        }
                    }
                }
            } else {
                let index = target
                    .boundary_index()
                    .expect("non-wire ordered targets are boundary targets");
                let Some(boundary) = network.ordered_boundary_target(index) else {
                    self.rollback_ordered(network, events);
                    return Err(OrderedPropagationError::InvalidBoundary(index));
                };
                let mut emit = true;
                let input = if let Some(node) = boundary.fast_input_node.filter(|node| {
                    self.fast_input_enabled
                        .get(node.index())
                        .copied()
                        .unwrap_or(false)
                }) {
                    let index = node.index();
                    let Some(input) = self.fast_inputs.get(index).copied() else {
                        self.rollback_ordered(network, events);
                        return Err(OrderedPropagationError::InvalidBoundary(index));
                    };
                    let Some(emitted_epoch) = self.fast_input_emitted_epochs.get_mut(index) else {
                        self.rollback_ordered(network, events);
                        return Err(OrderedPropagationError::InvalidBoundary(index));
                    };
                    let Some(dirty_epoch) = self.fast_input_dirty_epochs.get_mut(index) else {
                        self.rollback_ordered(network, events);
                        return Err(OrderedPropagationError::InvalidBoundary(index));
                    };
                    if *emitted_epoch == self.fast_input_epoch
                        && *dirty_epoch != self.fast_input_epoch
                    {
                        emit = false;
                        None
                    } else {
                        *emitted_epoch = self.fast_input_epoch;
                        *dirty_epoch = 0;
                        Some(input)
                    }
                } else {
                    None
                };
                if emit {
                    events.push(OrderedWireEvent::Boundary {
                        target: index as u32,
                        input,
                    });
                }
            }
            loop {
                if current_frame.next != current_frame.end {
                    let index = current_frame.next;
                    current_frame.next += 1;
                    let Some(next) = network.ordered_wire_target(index) else {
                        let error = OrderedPropagationError::InvalidOrderedTarget(index);
                        self.rollback_ordered(network, events);
                        return Err(error);
                    };
                    target = next;
                    break;
                }
                let Some(parent) = self.ordered_stack.pop() else {
                    return Ok(());
                };
                current_frame = parent;
            }
        }
    }

    fn rollback_ordered(&mut self, network: &StaticNetwork, events: &[OrderedWireEvent]) {
        for event in events.iter().rev() {
            if let OrderedWireEvent::Wire(transition) = event {
                let new_contribution = transition.new_power.saturating_sub(1);
                let old_contribution = transition.old_power.saturating_sub(1);
                if new_contribution != old_contribution {
                    for dependent in network.wire_dependents(transition.wire) {
                        self.update_predecessor_bucket(
                            *dependent,
                            new_contribution,
                            old_contribution,
                        );
                    }
                }
                self.wire_powers[transition.wire.index()] = transition.old_power;
            }
        }
        self.wire_inputs_dirty.fill(true);
    }

    fn rebuild_all_wires(&mut self, network: &StaticNetwork) {
        self.queue.clear();
        for index in 0..network.wire_count() {
            let wire = NetworkWireId(index as u32);
            let power = self.direct_wire_power(network, wire);
            self.wire_powers[index] = power;
            if power > 0 {
                self.queue.push_back(wire);
            }
        }
        while let Some(source) = self.queue.pop_front() {
            let propagated = self.wire_powers[source.index()].saturating_sub(1);
            if propagated == 0 {
                continue;
            }
            for target in network.wire_dependents(source) {
                if self.wire_powers[target.index()] < propagated {
                    self.wire_powers[target.index()] = propagated;
                    self.queue.push_back(*target);
                }
            }
        }
    }

    fn rebuild_predecessor_buckets(&mut self, network: &StaticNetwork) {
        self.wire_predecessor_counts.fill([0; 16]);
        self.wire_predecessor_masks.fill(0);
        for source_index in 0..network.wire_count() {
            let source = NetworkWireId(source_index as u32);
            let contribution = self.wire_powers[source_index].saturating_sub(1);
            if contribution == 0 {
                continue;
            }
            for target in network.wire_dependents(source) {
                self.increment_predecessor_bucket(*target, contribution);
            }
        }
    }

    fn update_predecessor_bucket(&mut self, wire: NetworkWireId, old: u8, new: u8) {
        if old > 0 {
            let count = &mut self.wire_predecessor_counts[wire.index()][old as usize];
            *count = count
                .checked_sub(1)
                .expect("wire predecessor bucket underflow");
            if *count == 0 {
                self.wire_predecessor_masks[wire.index()] &= !(1u16 << old);
            }
        }
        if new > 0 {
            self.increment_predecessor_bucket(wire, new);
        }
    }

    fn increment_predecessor_bucket(&mut self, wire: NetworkWireId, power: u8) {
        let count = &mut self.wire_predecessor_counts[wire.index()][power as usize];
        *count = count
            .checked_add(1)
            .expect("validated wire predecessor count exceeds u8");
        self.wire_predecessor_masks[wire.index()] |= 1u16 << power;
    }

    fn direct_wire_power(&self, network: &StaticNetwork, wire: NetworkWireId) -> u8 {
        self.direct_wire_powers
            .get(wire.index())
            .copied()
            .unwrap_or_else(|| self.scan_direct_wire_power(network, wire))
    }

    fn scan_direct_wire_power(&self, network: &StaticNetwork, wire: NetworkWireId) -> u8 {
        network
            .wire_sources(wire)
            .iter()
            .filter_map(|source| self.outputs.get(source.source.index()))
            .copied()
            .max()
            .unwrap_or(0)
    }

    fn rebuild_direct_wire_powers(&mut self, network: &StaticNetwork) {
        for index in 0..network.wire_count() {
            let wire = NetworkWireId(index as u32);
            self.direct_wire_powers[index] = self.scan_direct_wire_power(network, wire);
        }
    }

    fn node_input_power(
        &self,
        network: &StaticNetwork,
        node: NetworkNodeId,
    ) -> NetworkInputPower {
        let mut result = network
            .node(node)
            .map_or_else(NetworkInputPower::default, |node| node.constant_input);
        for port in network.input_ports(node) {
            let power = port
                .node_id()
                .and_then(|source| self.outputs.get(source.index()).copied())
                .map(|power| power.saturating_sub(port.attenuation))
                .or_else(|| port.wire_id().and_then(|wire| self.wire_power(wire)))
                .unwrap_or(0);
            match port.kind {
                NetworkInputKind::Default => result.default = result.default.max(power),
                NetworkInputKind::Side => result.side = result.side.max(power),
            }
        }
        result
    }

    fn update_fast_wire_inputs(
        &mut self,
        network: &StaticNetwork,
        wire: NetworkWireId,
        _power: u8,
    ) {
        for target in network.wire_targets(wire) {
            if target.kind != NetworkInputKind::Default
                || !network.node_has_fast_input(target.target)
            {
                continue;
            }
            let index = target.target.index();
            if !self.fast_input_enabled[index] {
                continue;
            }
            let next = self.node_input_power(network, target.target);
            if self.fast_inputs[index] != next {
                self.fast_inputs[index] = next;
                self.fast_input_dirty_epochs[index] = self.fast_input_epoch;
            }
        }
    }

    pub(in crate::compiled) fn invalidate_fast_inputs(
        &mut self,
        network: &StaticNetwork,
        positions: &rustc_hash::FxHashSet<BlockPos>,
    ) {
        for pos in positions {
            for wire in network.wire_starts_for_change(*pos) {
                for target in network.wire_targets(wire) {
                    if let Some(enabled) = self.fast_input_enabled.get_mut(target.target.index()) {
                        *enabled = false;
                    }
                }
                for target in network.ordered_wire_targets(wire) {
                    let Some(boundary) = target
                        .boundary_index()
                        .and_then(|index| network.ordered_boundary_target(index))
                    else {
                        continue;
                    };
                    if let Some(node) = boundary.fast_input_node
                        && let Some(enabled) = self.fast_input_enabled.get_mut(node.index())
                    {
                        *enabled = false;
                    }
                }
            }
        }
    }

    pub(in crate::compiled) fn disable_all_fast_inputs(&mut self) {
        self.fast_input_enabled.fill(false);
    }

    fn refresh_fast_inputs(&mut self, network: &StaticNetwork) {
        for index in 0..network.node_count() {
            let node = NetworkNodeId(index as u32);
            if network.node_has_fast_input(node) {
                self.fast_inputs[index] = self.node_input_power(network, node);
            }
        }
    }

    fn wire_target_power(&self, network: &StaticNetwork, wire: NetworkWireId) -> u8 {
        let mask = self.wire_predecessor_masks[wire.index()];
        let predecessor_power = if mask == 0 {
            0
        } else {
            (u16::BITS - 1 - mask.leading_zeros()) as u8
        };
        predecessor_power.max(self.direct_wire_power(network, wire))
    }
}

#[cfg(test)]
mod tests;
