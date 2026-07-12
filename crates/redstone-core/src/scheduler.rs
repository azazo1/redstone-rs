use std::array;
use std::collections::{BTreeMap, VecDeque};

use rustc_hash::FxHashSet;

use crate::{BlockKindId, BlockPos, GameTick, ScheduledTick, TickPriority};

const PRIORITY_COUNT: usize = 7;
const WHEEL_SIZE: usize = 256;

type TickQueues = [VecDeque<ScheduledTick>; PRIORITY_COUNT];

struct WheelSlot {
    tick: Option<GameTick>,
    queues: TickQueues,
}

impl WheelSlot {
    fn new() -> Self {
        Self {
            tick: None,
            queues: array::from_fn(|_| VecDeque::new()),
        }
    }

    fn is_empty(&self) -> bool {
        self.queues.iter().all(VecDeque::is_empty)
    }
}

pub(crate) struct ScheduledTickQueue {
    wheel: Vec<WheelSlot>,
    overflow: BTreeMap<GameTick, TickQueues>,
    keys: FxHashSet<(BlockPos, BlockKindId)>,
    len: usize,
}

impl ScheduledTickQueue {
    pub(crate) fn new() -> Self {
        Self {
            wheel: (0..WHEEL_SIZE).map(|_| WheelSlot::new()).collect(),
            overflow: BTreeMap::new(),
            keys: FxHashSet::default(),
            len: 0,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn contains(&self, pos: BlockPos, block: BlockKindId) -> bool {
        self.keys.contains(&(pos, block))
    }

    pub(crate) fn snapshot(&self) -> Vec<ScheduledTick> {
        let mut ticks = self
            .wheel
            .iter()
            .flat_map(|slot| slot.queues.iter())
            .chain(self.overflow.values().flat_map(|queues| queues.iter()))
            .flat_map(|queue| queue.iter().copied())
            .collect::<Vec<_>>();
        ticks.sort_unstable();
        ticks
    }

    pub(crate) fn insert(&mut self, tick: ScheduledTick) -> bool {
        if !self.keys.insert((tick.pos, tick.block)) {
            return false;
        }

        let priority = priority_index(tick.priority);
        if let Some(queues) = self.overflow.get_mut(&tick.trigger_tick) {
            insert_ordered(&mut queues[priority], tick);
        } else {
            let index = wheel_index(tick.trigger_tick);
            let slot = &mut self.wheel[index];
            if slot.tick.is_none() || slot.tick == Some(tick.trigger_tick) {
                slot.tick = Some(tick.trigger_tick);
                insert_ordered(&mut slot.queues[priority], tick);
            } else {
                let queue = &mut self
                    .overflow
                    .entry(tick.trigger_tick)
                    .or_insert_with(|| array::from_fn(|_| VecDeque::new()))[priority];
                insert_ordered(queue, tick);
            }
        }
        self.len += 1;
        true
    }

    pub(crate) fn due_batch(
        &self,
        current_tick: GameTick,
        limit: usize,
    ) -> (Vec<ScheduledTick>, bool) {
        let mut due_ticks = self
            .wheel
            .iter()
            .filter_map(|slot| slot.tick.filter(|tick| *tick <= current_tick))
            .chain(self.overflow.range(..=current_tick).map(|(tick, _)| *tick))
            .collect::<Vec<_>>();
        due_ticks.sort_unstable();

        let mut due = Vec::with_capacity(limit.min(self.len));
        let mut has_more = false;
        for trigger_tick in due_ticks {
            let queues = self
                .queues(trigger_tick)
                .expect("到期计划刻必须存在于时间轮中");
            for queue in queues {
                for tick in queue {
                    if due.len() == limit {
                        has_more = true;
                        return (due, has_more);
                    }
                    due.push(*tick);
                }
            }
        }
        (due, has_more)
    }

    pub(crate) fn start_execution(&mut self, tick: ScheduledTick) {
        let index = wheel_index(tick.trigger_tick);
        let use_wheel = self.wheel[index].tick == Some(tick.trigger_tick);
        let queues = if use_wheel {
            &mut self.wheel[index].queues
        } else {
            self.overflow
                .get_mut(&tick.trigger_tick)
                .expect("执行的计划刻必须仍在时间轮中")
        };
        let queued = queues[priority_index(tick.priority)]
            .pop_front()
            .expect("执行的计划刻必须位于优先级队首");
        assert_eq!(queued, tick, "计划刻执行顺序必须保持稳定");

        if use_wheel {
            if self.wheel[index].is_empty() {
                self.wheel[index].tick = None;
            }
        } else if queues.iter().all(VecDeque::is_empty) {
            self.overflow.remove(&tick.trigger_tick);
        }
        self.keys.remove(&(tick.pos, tick.block));
        self.len -= 1;
    }

    fn queues(&self, tick: GameTick) -> Option<&TickQueues> {
        let slot = &self.wheel[wheel_index(tick)];
        if slot.tick == Some(tick) {
            Some(&slot.queues)
        } else {
            self.overflow.get(&tick)
        }
    }
}

fn wheel_index(tick: GameTick) -> usize {
    (tick.0 % WHEEL_SIZE as u64) as usize
}

fn priority_index(priority: TickPriority) -> usize {
    (priority.value() + 3) as usize
}

fn insert_ordered(queue: &mut VecDeque<ScheduledTick>, tick: ScheduledTick) {
    if queue.back().is_none_or(|queued| *queued <= tick) {
        queue.push_back(tick);
        return;
    }
    let index = queue
        .iter()
        .position(|queued| *queued > tick)
        .expect("非尾部计划刻必须存在有序插入位置");
    queue.insert(index, tick);
}

#[cfg(test)]
mod tests {
    use super::ScheduledTickQueue;
    use crate::{BlockKindId, BlockPos, GameTick, ScheduledTick, TickPriority};

    fn tick(
        pos: i32,
        trigger_tick: u64,
        priority: TickPriority,
        sub_tick_order: i64,
    ) -> ScheduledTick {
        ScheduledTick {
            block: BlockKindId(1),
            pos: BlockPos::new(pos, 0, 0),
            trigger_tick: GameTick(trigger_tick),
            priority,
            sub_tick_order,
        }
    }

    fn tick_with_block(
        pos: i32,
        block: u32,
        trigger_tick: u64,
        priority: TickPriority,
        sub_tick_order: i64,
    ) -> ScheduledTick {
        ScheduledTick {
            block: BlockKindId(block),
            pos: BlockPos::new(pos, 0, 0),
            trigger_tick: GameTick(trigger_tick),
            priority,
            sub_tick_order,
        }
    }

    #[test]
    fn orders_by_tick_priority_and_sub_tick_order() {
        let mut queue = ScheduledTickQueue::new();
        let ticks = [
            tick(3, 2, TickPriority::Normal, 3),
            tick(4, 1, TickPriority::Normal, 4),
            tick(2, 1, TickPriority::Normal, 2),
            tick(1, 1, TickPriority::VeryHigh, 1),
        ];
        for tick in ticks {
            assert!(queue.insert(tick));
        }

        let (due, has_more) = queue.due_batch(GameTick(2), usize::MAX);

        assert_eq!(due, [ticks[3], ticks[2], ticks[1], ticks[0]]);
        assert!(!has_more);
    }

    #[test]
    fn orders_all_seven_priority_levels() {
        let priorities = [
            TickPriority::ExtremelyHigh,
            TickPriority::VeryHigh,
            TickPriority::High,
            TickPriority::Normal,
            TickPriority::Low,
            TickPriority::VeryLow,
            TickPriority::ExtremelyLow,
        ];
        let mut queue = ScheduledTickQueue::new();
        for (index, priority) in priorities.iter().copied().rev().enumerate() {
            assert!(queue.insert(tick(index as i32, 5, priority, index as i64)));
        }

        let (due, has_more) = queue.due_batch(GameTick(5), usize::MAX);
        let actual = due
            .iter()
            .map(|scheduled| scheduled.priority)
            .collect::<Vec<_>>();

        assert_eq!(actual, priorities);
        assert!(!has_more);
    }

    #[test]
    fn orders_sub_tick_values_within_every_priority_level() {
        let priorities = [
            TickPriority::ExtremelyHigh,
            TickPriority::VeryHigh,
            TickPriority::High,
            TickPriority::Normal,
            TickPriority::Low,
            TickPriority::VeryLow,
            TickPriority::ExtremelyLow,
        ];
        for priority in priorities {
            let mut queue = ScheduledTickQueue::new();
            for (pos, sub_tick_order) in [(0, 30), (1, 10), (2, 20)] {
                assert!(queue.insert(tick(pos, 5, priority, sub_tick_order)));
            }

            let due = queue.due_batch(GameTick(5), usize::MAX).0;
            let actual = due
                .iter()
                .map(|scheduled| scheduled.sub_tick_order)
                .collect::<Vec<_>>();
            assert_eq!(actual, [10, 20, 30], "priority {priority:?}");
        }
    }

    #[test]
    fn deduplicates_only_by_position_and_block_kind() {
        let first = tick_with_block(1, 1, 5, TickPriority::Normal, 10);
        let cases = [
            (
                "different trigger tick",
                tick_with_block(1, 1, 6, TickPriority::Normal, 10),
                false,
            ),
            (
                "different priority",
                tick_with_block(1, 1, 5, TickPriority::High, 10),
                false,
            ),
            (
                "different sub tick order",
                tick_with_block(1, 1, 5, TickPriority::Normal, 11),
                false,
            ),
            (
                "different position",
                tick_with_block(2, 1, 5, TickPriority::Normal, 10),
                true,
            ),
            (
                "different block kind",
                tick_with_block(1, 2, 5, TickPriority::Normal, 10),
                true,
            ),
        ];

        for (case, candidate, expected) in cases {
            let mut queue = ScheduledTickQueue::new();
            assert!(queue.insert(first));
            assert_eq!(queue.insert(candidate), expected, "{case}");
        }
    }

    #[test]
    fn keeps_snapshot_ticks_queued_until_execution_starts() {
        let mut queue = ScheduledTickQueue::new();
        let first = tick(1, 0, TickPriority::Normal, 0);
        let second = tick(2, 0, TickPriority::Normal, 1);
        assert!(queue.insert(first));
        assert!(queue.insert(second));

        let (due, _) = queue.due_batch(GameTick(0), usize::MAX);
        assert!(!queue.insert(tick(1, 0, TickPriority::Normal, 2)));
        queue.start_execution(due[0]);
        let rescheduled = tick(1, 0, TickPriority::Normal, 2);
        assert!(queue.insert(rescheduled));

        assert_eq!(due, [first, second]);
        assert_eq!(
            queue.due_batch(GameTick(0), usize::MAX).0,
            [second, rescheduled]
        );
    }

    #[test]
    fn preserves_colliding_ticks_in_overflow() {
        let mut queue = ScheduledTickQueue::new();
        let first = tick(1, 1, TickPriority::Normal, 0);
        let second = tick(2, 257, TickPriority::Normal, 1);
        assert!(queue.insert(first));
        assert!(queue.insert(second));

        let (early, _) = queue.due_batch(GameTick(1), usize::MAX);
        assert_eq!(early, [first]);
        queue.start_execution(first);
        let (late, _) = queue.due_batch(GameTick(257), usize::MAX);
        assert_eq!(late, [second]);
    }

    #[test]
    fn reports_more_due_without_removing_the_remainder() {
        let mut queue = ScheduledTickQueue::new();
        let first = tick(1, 1, TickPriority::Normal, 0);
        let second = tick(2, 1, TickPriority::Normal, 1);
        assert!(queue.insert(first));
        assert!(queue.insert(second));

        let (due, has_more) = queue.due_batch(GameTick(1), 1);
        assert_eq!(due, [first]);
        assert!(has_more);
        queue.start_execution(first);
        let (remaining, _) = queue.due_batch(GameTick(1), usize::MAX);
        assert_eq!(remaining, [second]);
    }

    #[test]
    fn snapshot_is_sorted_and_does_not_consume_ticks() {
        let mut queue = ScheduledTickQueue::new();
        let first = tick(1, 1, TickPriority::VeryHigh, 2);
        let second = tick(2, 1, TickPriority::Normal, 3);
        let third = tick(3, 300, TickPriority::High, 1);
        for tick in [third, second, first] {
            assert!(queue.insert(tick));
        }

        assert_eq!(queue.snapshot(), [first, second, third]);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue.due_batch(GameTick(1), usize::MAX).0, [first, second]);
    }
}
