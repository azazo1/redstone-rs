use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::{BlockEntityData, BlockKindId, BlockPos, BlockStateId, Direction, GameTick};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[repr(i8)]
pub enum TickPriority {
    ExtremelyHigh = -3,
    VeryHigh = -2,
    High = -1,
    Normal = 0,
    Low = 1,
    VeryLow = 2,
    ExtremelyLow = 3,
}

impl TickPriority {
    pub const fn value(self) -> i8 {
        self as i8
    }
}

impl Ord for TickPriority {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value().cmp(&other.value())
    }
}

impl PartialOrd for TickPriority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ScheduledTick {
    pub block: BlockKindId,
    pub pos: BlockPos,
    pub trigger_tick: GameTick,
    pub priority: TickPriority,
    pub sub_tick_order: i64,
}

impl Ord for ScheduledTick {
    fn cmp(&self, other: &Self) -> Ordering {
        self.trigger_tick
            .cmp(&other.trigger_tick)
            .then_with(|| self.priority.cmp(&other.priority))
            .then_with(|| self.sub_tick_order.cmp(&other.sub_tick_order))
            .then_with(|| self.pos.cmp(&other.pos))
            .then_with(|| self.block.cmp(&other.block))
    }
}

impl PartialOrd for ScheduledTick {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BlockEvent {
    pub pos: BlockPos,
    pub block: BlockKindId,
    pub param_a: i32,
    pub param_b: i32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NeighborUpdate {
    pub pos: BlockPos,
    pub source_pos: BlockPos,
    pub source_block: BlockKindId,
    pub orientation: Option<u8>,
    pub moved_by_piston: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeferredBlockEntityUpdate {
    Keep,
    Remove,
    Set(BlockEntityData),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredBlockChange {
    pub pos: BlockPos,
    pub state: BlockStateId,
    pub cause: String,
    pub block_entity: DeferredBlockEntityUpdate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeferredRuleTask {
    pub kind: &'static str,
    pub pos: BlockPos,
    pub param_a: i32,
    pub param_b: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NeighborTask {
    Single(NeighborUpdate),
    Multi {
        source_pos: BlockPos,
        source_block: BlockKindId,
        skip_direction: Option<Direction>,
        orientation: Option<u8>,
        next_index: usize,
    },
    ScheduleTickAfterNeighbors {
        pos: BlockPos,
        block: BlockKindId,
        delay: u64,
        priority: TickPriority,
    },
    SetBlockAndUpdateNeighborsAfterNeighbors {
        pos: BlockPos,
        state: BlockStateId,
        cause: Box<str>,
        source_block: BlockKindId,
    },
    ApplyBlockChangesAfterNeighbors {
        changes: Box<[DeferredBlockChange]>,
        follow_up: Box<[NeighborTask]>,
    },
    RunRuleTaskAfterNeighbors(Box<DeferredRuleTask>),
}
