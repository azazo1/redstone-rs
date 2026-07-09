use std::collections::BTreeMap;

use fastnbt::to_bytes;
use redstone_rs::{
    core::{BlockKind, BlockState, Position, Snapshot, SnapshotBlock, TraceEvent, TraceKind},
    io::{diff_snapshots, diff_traces, SimulationTrace, TestVector},
    InputAction,
};
use redstone_rs::api::InputOperation;
use serde::Serialize;

fn snapshot(tick: u64, blocks: Vec<SnapshotBlock>) -> Snapshot {
    Snapshot { game_tick: tick, blocks }
}

fn block(position: Position, kind: BlockKind) -> SnapshotBlock {
    SnapshotBlock {
        position,
        state: BlockState::new(kind),
        block_entity: None,
    }
}

#[derive(Serialize)]
struct StructureFixture {
    palette: Vec<PaletteFixture>,
    blocks: Vec<StructureBlockFixture>,
}

#[derive(Serialize)]
struct PaletteFixture {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct StructureBlockFixture {
    pos: Vec<i32>,
    state: usize,
}

#[test]
fn snapshot_diff_reports_missing_and_changed_blocks_at_the_correct_tick() {
    let position = Position::new(1, 2, 3);
    let expected = vec![snapshot(4, vec![block(position, BlockKind::RedstoneWire)])];
    let actual = vec![snapshot(4, vec![])];

    let differences = diff_snapshots(&expected, &actual);

    assert_eq!(differences.len(), 1);
    assert_eq!(differences[0].tick, 4);
    assert_eq!(differences[0].position, position);
    assert_eq!(differences[0].expected.as_ref().map(|block| block.state.kind), Some(BlockKind::RedstoneWire));
    assert_eq!(differences[0].actual, None);
}

#[test]
fn trace_diff_reports_event_order_even_when_snapshots_match() {
    let position = Position::new(1, 2, 3);
    let frame = snapshot(1, vec![block(position, BlockKind::Solid)]);
    let event = TraceEvent {
        tick: 1,
        sequence: 0,
        kind: TraceKind::NeighborUpdate,
        position,
        before: None,
        after: Some(BlockState::new(BlockKind::Solid)),
        cause: "neighbor:RedstoneWire".to_owned(),
    };
    let expected = SimulationTrace {
        frames: vec![frame.clone()],
        events: vec![event.clone()],
    };
    let actual = SimulationTrace {
        frames: vec![frame],
        events: vec![TraceEvent {
            kind: TraceKind::BlockEvent,
            ..event
        }],
    };

    let difference = diff_traces(&expected, &actual);

    assert!(difference.snapshots.is_empty());
    assert_eq!(difference.events.len(), 1);
    assert_eq!(difference.events[0].index, 0);
    assert_eq!(difference.events[0].expected.as_ref().map(|event| event.kind), Some(TraceKind::NeighborUpdate));
    assert_eq!(difference.events[0].actual.as_ref().map(|event| event.kind), Some(TraceKind::BlockEvent));
}

#[tokio::test]
async fn test_vector_runs_relative_structure_and_filters_observation_area() {
    let root = std::env::temp_dir().join(format!("redstone-rs-vector-{}", std::process::id()));
    std::fs::create_dir_all(&root).expect("vector directory should create");
    let structure = StructureFixture {
        palette: vec![
            PaletteFixture {
                name: "minecraft:lever".to_owned(),
                properties: BTreeMap::new(),
            },
            PaletteFixture {
                name: "minecraft:redstone_wire".to_owned(),
                properties: BTreeMap::new(),
            },
        ],
        blocks: vec![
            StructureBlockFixture {
                pos: vec![0, 0, 0],
                state: 0,
            },
            StructureBlockFixture {
                pos: vec![1, 0, 0],
                state: 1,
            },
        ],
    };
    std::fs::write(root.join("machine.nbt"), to_bytes(&structure).expect("structure should serialize"))
        .expect("structure fixture should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: vec![redstone_rs::io::TimedAction {
            action: InputAction {
                tick: 1,
                operation: InputOperation::UseBlock {
                    position: Position::new(0, 0, 0),
                },
            },
        }],
        observe_min: Position::new(1, 0, 0),
        observe_max: Position::new(1, 0, 0),
        ticks: 1,
    };
    std::fs::write(root.join("case.json"), serde_json::to_vec(&vector).expect("vector should serialize"))
        .expect("vector fixture should write");

    let trace = TestVector::run_from_path(root.join("case.json"))
        .await
        .expect("vector should execute");
    std::fs::remove_dir_all(&root).expect("vector directory should remove");

    assert_eq!(trace.frames.len(), 1);
    assert_eq!(trace.frames[0].blocks.len(), 1);
    assert_eq!(trace.frames[0].blocks[0].state.power(), 15);
    assert!(!trace.events.is_empty());
    assert!(trace.events.iter().all(|event| event.position == Position::new(1, 0, 0)));
}
