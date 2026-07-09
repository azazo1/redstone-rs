use redstone_rs::{
    core::BlockKind,
    io::{StructureBlock, StructureInput},
    BlockState, Direction, InputAction, Position, SimulationSession,
};
use redstone_rs::api::InputOperation;

#[tokio::test]
async fn lever_drives_wire_on_the_next_game_tick() {
    let lever = Position::new(0, 0, 0);
    let wire = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: lever,
                state: BlockState::new(BlockKind::Lever).with_facing(Direction::East),
            },
            StructureBlock {
                position: wire,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
        ],
    })
    .await
    .expect("structure should load");

    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::UseBlock { position: lever },
        })
        .await;
    session.step().await.expect("simulation should step");

    let wire_state = session.world().state(wire);
    assert_eq!(wire_state.power(), 15);
}

#[tokio::test]
async fn redstone_wire_loses_one_power_level_per_segment() {
    let source = Position::new(0, 0, 0);
    let first = Position::new(1, 0, 0);
    let second = Position::new(2, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: first,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
            StructureBlock {
                position: second,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("simulation should step");

    assert_eq!(session.world().state(first).power(), 15);
    assert_eq!(session.world().state(second).power(), 14);
}

#[tokio::test]
async fn piston_uses_quasi_connectivity_from_above() {
    let piston = Position::new(0, 0, 0);
    let source = Position::new(1, 1, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::Piston).with_facing(Direction::East),
            },
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
        ],
    })
    .await
    .expect("structure should load");

    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::TriggerNeighborUpdate {
                position: piston,
                changed_block: BlockKind::Solid,
            },
        })
        .await;
    session.step().await.expect("simulation should step");

    assert!(session.world().state(piston).extended());
    assert_eq!(session.world().state(piston.offset(Direction::East)).kind, BlockKind::PistonHead);
}
