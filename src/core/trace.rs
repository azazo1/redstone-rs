use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::{BlockState, Position};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TraceKind {
    StateChange,
    ScheduledTick,
    BlockEvent,
    NeighborUpdate,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEvent {
    pub tick: u64,
    pub sequence: u64,
    pub kind: TraceKind,
    pub position: Position,
    pub before: Option<BlockState>,
    pub after: Option<BlockState>,
    pub cause: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceFilter {
    pub min: Option<Position>,
    pub max: Option<Position>,
}

impl TraceFilter {
    pub fn contains(&self, position: Position) -> bool {
        match (self.min, self.max) {
            (Some(min), Some(max)) => {
                (min.x..=max.x).contains(&position.x)
                    && (min.y..=max.y).contains(&position.y)
                    && (min.z..=max.z).contains(&position.z)
            }
            _ => true,
        }
    }
}

#[derive(Debug, Default)]
pub struct TraceRecorder {
    filter: Option<TraceFilter>,
    enabled: Option<BTreeSet<TraceKind>>,
    sequence: u64,
    events: Vec<TraceEvent>,
}

impl TraceRecorder {
    pub fn set_filter(&mut self, filter: Option<TraceFilter>, enabled: Option<BTreeSet<TraceKind>>) {
        self.filter = filter;
        self.enabled = enabled;
    }

    pub fn record(
        &mut self,
        tick: u64,
        kind: TraceKind,
        position: Position,
        before: Option<BlockState>,
        after: Option<BlockState>,
        cause: impl Into<String>,
    ) {
        if self.filter.is_some_and(|filter| !filter.contains(position)) {
            return;
        }
        if self.enabled.as_ref().is_some_and(|enabled| !enabled.contains(&kind)) {
            return;
        }
        self.events.push(TraceEvent {
            tick,
            sequence: self.sequence,
            kind,
            position,
            before,
            after,
            cause: cause.into(),
        });
        self.sequence = self.sequence.wrapping_add(1);
    }

    pub fn events(&self) -> &[TraceEvent] {
        &self.events
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.sequence = 0;
    }
}
