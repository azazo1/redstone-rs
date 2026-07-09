use redstone_rs::{
    core::{BlockEvent, NeighborUpdate, NeighborUpdater, Scheduler, TickPriority},
    BlockKind, Direction, Position,
};

#[test]
fn scheduled_ticks_use_priority_then_insertion_order_and_deduplicate() {
    let position = Position::new(1, 2, 3);
    let mut scheduler = Scheduler::default();
    scheduler.schedule(10, position, BlockKind::Repeater, 2, TickPriority::Normal);
    scheduler.schedule(10, Position::new(2, 2, 3), BlockKind::Comparator, 2, TickPriority::High);
    scheduler.schedule(10, position, BlockKind::Repeater, 2, TickPriority::ExtremelyLow);

    let first = scheduler.pop_due(12).expect("high priority tick");
    let second = scheduler.pop_due(12).expect("normal priority tick");

    assert_eq!(first.block, BlockKind::Comparator);
    assert_eq!(second.block, BlockKind::Repeater);
    assert!(scheduler.pop_due(12).is_none());
}

#[test]
fn scheduler_retains_the_high_water_mark_after_events_are_processed() {
    let mut scheduler = Scheduler::default();
    scheduler.schedule(0, Position::new(0, 0, 0), BlockKind::Repeater, 1, TickPriority::Normal);
    scheduler.enqueue_block_event(BlockEvent {
        position: Position::new(1, 0, 0),
        block: BlockKind::Piston,
        action: 0,
        parameter: 0,
    });

    assert_eq!(scheduler.peak_pending_len(), 2);
    assert!(scheduler.pop_due(1).is_some());
    assert!(scheduler.pop_block_event().is_some());
    assert_eq!(scheduler.pending_len(), 0);
    assert_eq!(scheduler.peak_pending_len(), 2);
}

#[test]
fn neighbor_updates_follow_java_default_direction_order() {
    let origin = Position::new(4, 8, 12);
    let mut updater = NeighborUpdater::new(32);
    assert!(updater.enqueue(NeighborUpdate::Multi {
        source: origin,
        changed_block: BlockKind::Solid,
        skip: None,
        next_index: 0,
    }));
    assert!(updater.begin_if_idle());

    let positions = std::iter::from_fn(|| updater.take_next_event().map(|event| event.0)).collect::<Vec<_>>();

    let expected = Direction::NEIGHBOR_ORDER.map(|direction| origin.offset(direction));
    assert_eq!(positions, expected);
    updater.finish();
}
