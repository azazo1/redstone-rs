use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};

use serde::{Deserialize, Serialize};

use super::{BlockKind, Position};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(i8)]
pub enum TickPriority {
    ExtremelyHigh = -3,
    VeryHigh = -2,
    High = -1,
    #[default]
    Normal = 0,
    Low = 1,
    VeryLow = 2,
    ExtremelyLow = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScheduledTick {
    pub due_tick: u64,
    pub priority: TickPriority,
    pub sub_tick_order: u64,
    pub position: Position,
    pub block: BlockKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BlockEvent {
    pub position: Position,
    pub block: BlockKind,
    pub action: u8,
    pub parameter: u8,
}

#[derive(Debug, Default)]
pub struct Scheduler {
    next_sub_tick: u64,
    scheduled: BinaryHeap<Reverse<ScheduledTick>>,
    scheduled_keys: BTreeSet<(BlockKind, Position)>,
    block_events: VecDeque<BlockEvent>,
    block_event_keys: BTreeSet<BlockEvent>,
}

impl Scheduler {
    pub fn schedule(
        &mut self,
        current_tick: u64,
        position: Position,
        block: BlockKind,
        delay: u64,
        priority: TickPriority,
    ) {
        if !self.scheduled_keys.insert((block, position)) {
            return;
        }
        let tick = ScheduledTick {
            due_tick: current_tick.saturating_add(delay),
            priority,
            sub_tick_order: self.next_sub_tick,
            position,
            block,
        };
        self.next_sub_tick = self.next_sub_tick.wrapping_add(1);
        self.scheduled.push(Reverse(tick));
    }

    pub fn pop_due(&mut self, current_tick: u64) -> Option<ScheduledTick> {
        let Reverse(next) = self.scheduled.peek().copied()?;
        if next.due_tick > current_tick {
            return None;
        }
        let Reverse(next) = self.scheduled.pop().expect("scheduled tick was present");
        self.scheduled_keys.remove(&(next.block, next.position));
        Some(next)
    }

    pub fn has_scheduled(&self, position: Position, block: BlockKind) -> bool {
        self.scheduled_keys.contains(&(block, position))
    }

    pub fn enqueue_block_event(&mut self, event: BlockEvent) {
        if self.block_event_keys.insert(event) {
            self.block_events.push_back(event);
        }
    }

    pub fn pop_block_event(&mut self) -> Option<BlockEvent> {
        let event = self.block_events.pop_front()?;
        self.block_event_keys.remove(&event);
        Some(event)
    }

    pub fn pending_len(&self) -> usize {
        self.scheduled.len() + self.block_events.len()
    }
}
