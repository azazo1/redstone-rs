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
async fn external_power_operation_drives_and_releases_pressure_plate_signal() {
    let plate = Position::new(0, 0, 0);
    let wire = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: plate,
                state: BlockState::new(BlockKind::PressurePlate),
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
            operation: InputOperation::SetPowered {
                position: plate,
                powered: true,
            },
        })
        .await;
    session.step().await.expect("pressure plate should power wire");
    assert_eq!(session.world().state(wire).power(), 15);

    session
        .apply(InputAction {
            tick: 2,
            operation: InputOperation::SetPowered {
                position: plate,
                powered: false,
            },
        })
        .await;
    session.step().await.expect("pressure plate should release wire");
    assert_eq!(session.world().state(wire).power(), 0);
}

#[tokio::test]
async fn external_signal_preserves_daylight_detector_power_level() {
    let detector = Position::new(0, 0, 0);
    let wire = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: detector,
                state: BlockState::new(BlockKind::DaylightDetector),
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
            operation: InputOperation::SetSignal {
                position: detector,
                signal: 9,
            },
        })
        .await;
    session.step().await.expect("daylight detector should power wire");

    assert_eq!(session.world().state(wire).power(), 9);
}

#[tokio::test]
async fn target_block_signal_expires_after_twenty_game_ticks() {
    let target = Position::new(0, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![StructureBlock {
            position: target,
            state: BlockState::new(BlockKind::Target),
        }],
    })
    .await
    .expect("structure should load");

    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::SetSignal {
                position: target,
                signal: 15,
            },
        })
        .await;
    session.step().await.expect("target should activate");
    assert_eq!(session.world().state(target).power(), 15);
    for _ in 0..19 {
        session.step().await.expect("target timeout should advance");
    }
    assert_eq!(session.world().state(target).power(), 0);
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
async fn redstone_wire_connects_upward_over_adjacent_conductor() {
    let source = Position::new(0, 0, 0);
    let low_wire = Position::new(1, 0, 0);
    let step = Position::new(2, 0, 0);
    let high_wire = Position::new(2, 1, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: low_wire,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
            StructureBlock {
                position: step,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: high_wire,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("wire network should stabilize");

    assert_eq!(session.world().state(low_wire).power(), 15);
    assert_eq!(session.world().state(high_wire).power(), 14);
}

#[tokio::test]
async fn repeater_applies_two_game_tick_delay_and_side_lock() {
    let source = Position::new(0, 0, 0);
    let repeater = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: repeater,
                state: BlockState::new(BlockKind::Repeater).with_facing(Direction::East).with_delay(1),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("first delay tick should run");
    assert!(!session.world().state(repeater).powered());
    session.step().await.expect("second delay tick should run");
    assert!(session.world().state(repeater).powered());

    session
        .apply(InputAction {
            tick: 3,
            operation: InputOperation::SetBlock {
                position: Position::new(1, 0, 1),
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
        })
        .await;
    session.step().await.expect("side source should lock repeater");
    assert!(session.world().state(repeater).locked());
}

#[tokio::test]
async fn comparator_preserves_container_signal_strength() {
    let container = Position::new(0, 0, 0);
    let comparator = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: container,
                state: BlockState::new(BlockKind::Container),
            },
            StructureBlock {
                position: comparator,
                state: BlockState::new(BlockKind::Comparator).with_facing(Direction::East),
            },
        ],
    })
    .await
    .expect("structure should load");

    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::SetComparatorSignal {
                position: container,
                signal: 10,
            },
        })
        .await;
    session.step().await.expect("comparator should update");

    assert_eq!(session.world().state(comparator).power(), 10);
    assert!(session.world().state(comparator).powered());
}

#[tokio::test]
async fn comparator_subtract_mode_applies_side_signal_strength() {
    let container = Position::new(0, 0, 0);
    let comparator = Position::new(1, 0, 0);
    let side_wire = Position::new(1, 0, 1);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: container,
                state: BlockState::new(BlockKind::Container),
            },
            StructureBlock {
                position: comparator,
                state: BlockState::new(BlockKind::Comparator)
                    .with_facing(Direction::East)
                    .with_mode(redstone_rs::core::ComparatorMode::Subtract),
            },
            StructureBlock {
                position: side_wire,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
        ],
    })
    .await
    .expect("structure should load");

    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::SetComparatorSignal {
                position: container,
                signal: 10,
            },
        })
        .await;
    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::SetBlock {
                position: side_wire,
                state: BlockState::new(BlockKind::RedstoneWire).with_powered(true).with_power(5),
            },
        })
        .await;
    session.step().await.expect("comparator should update");

    assert_eq!(session.world().state(comparator).power(), 5);
}

#[tokio::test]
async fn observer_emits_a_two_game_tick_pulse() {
    let source = Position::new(0, 0, 0);
    let observer = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: observer,
                state: BlockState::new(BlockKind::Observer).with_facing(Direction::West),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("first observer delay tick should run");
    assert!(!session.world().state(observer).powered());
    session.step().await.expect("observer should power on");
    assert!(session.world().state(observer).powered());
    session.step().await.expect("observer pulse should remain active");
    assert!(session.world().state(observer).powered());
    session.step().await.expect("observer should power off");
    assert!(!session.world().state(observer).powered());
}

#[tokio::test]
async fn redstone_torch_inverts_support_power_after_two_game_ticks() {
    let support = Position::new(0, 0, 0);
    let torch = Position::new(0, 1, 0);
    let lever = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: support,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: torch,
                state: BlockState::new(BlockKind::RedstoneTorch),
            },
            StructureBlock {
                position: lever,
                state: BlockState::new(BlockKind::Lever),
            },
        ],
    })
    .await
    .expect("structure should load");

    assert!(session.world().state(torch).powered());
    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::UseBlock { position: lever },
        })
        .await;
    session.step().await.expect("first torch delay tick should run");
    assert!(session.world().state(torch).powered());
    session.step().await.expect("torch should turn off");
    assert!(!session.world().state(torch).powered());

    session
        .apply(InputAction {
            tick: 3,
            operation: InputOperation::UseBlock { position: lever },
        })
        .await;
    session.step().await.expect("first restore delay tick should run");
    assert!(!session.world().state(torch).powered());
    session.step().await.expect("torch should turn on");
    assert!(session.world().state(torch).powered());
}

#[tokio::test]
async fn redstone_lamp_turns_off_after_four_game_ticks_without_power() {
    let source = Position::new(0, 0, 0);
    let lamp = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: lamp,
                state: BlockState::new(BlockKind::Lamp),
            },
        ],
    })
    .await
    .expect("structure should load");

    assert!(session.world().state(lamp).powered());
    session
        .apply(InputAction {
            tick: 1,
            operation: InputOperation::SetBlock {
                position: source,
                state: BlockState::new(BlockKind::Air),
            },
        })
        .await;
    for _ in 0..3 {
        session.step().await.expect("lamp delay tick should run");
        assert!(session.world().state(lamp).powered());
    }
    session.step().await.expect("lamp should turn off");
    assert!(!session.world().state(lamp).powered());
}

#[tokio::test]
async fn copper_bulb_toggles_only_on_power_rising_edges() {
    let source = Position::new(0, 0, 0);
    let bulb = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::Lever),
            },
            StructureBlock {
                position: bulb,
                state: BlockState::new(BlockKind::CopperBulb),
            },
        ],
    })
    .await
    .expect("structure should load");

    for tick in [1, 2, 3] {
        session
            .apply(InputAction {
                tick,
                operation: InputOperation::UseBlock { position: source },
            })
            .await;
        session.step().await.expect("bulb input should update");
    }

    assert!(session.world().state(bulb).powered());
    assert!(!session.world().state(bulb).lit());
}

#[tokio::test]
async fn redstone_torch_burns_out_after_eight_recent_turn_offs() {
    let support = Position::new(0, 0, 0);
    let torch = Position::new(0, 1, 0);
    let lever = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: support,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: torch,
                state: BlockState::new(BlockKind::RedstoneTorch),
            },
            StructureBlock {
                position: lever,
                state: BlockState::new(BlockKind::Lever),
            },
        ],
    })
    .await
    .expect("structure should load");

    for _ in 0..8 {
        let turn_on_tick = session.world().game_tick() + 1;
        session
            .apply(InputAction {
                tick: turn_on_tick,
                operation: InputOperation::UseBlock { position: lever },
            })
            .await;
        session.step().await.expect("torch input should change");
        session.step().await.expect("torch should turn off");

        let turn_off_tick = session.world().game_tick() + 1;
        session
            .apply(InputAction {
                tick: turn_off_tick,
                operation: InputOperation::UseBlock { position: lever },
            })
            .await;
        session.step().await.expect("torch input should reset");
        session.step().await.expect("torch should attempt to turn on");
    }

    assert!(!session.world().state(torch).powered());
    session.step().await.expect("burnout should persist");
    assert!(!session.world().state(torch).powered());
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
    assert_eq!(session.world().state(piston.offset(Direction::East)).kind, BlockKind::MovingPiston);
    for _ in 0..3 {
        session.step().await.expect("piston motion should advance");
    }
    assert_eq!(session.world().state(piston.offset(Direction::East)).kind, BlockKind::PistonHead);
}

#[tokio::test]
async fn slime_block_carries_adjacent_solid_block_without_carrying_honey() {
    let piston = Position::new(0, 0, 0);
    let slime = Position::new(1, 0, 0);
    let solid = Position::new(1, 1, 0);
    let honey = Position::new(1, 0, 1);
    let source = Position::new(0, -1, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::Piston).with_facing(Direction::East),
            },
            StructureBlock {
                position: slime,
                state: BlockState::new(BlockKind::SlimeBlock),
            },
            StructureBlock {
                position: solid,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: honey,
                state: BlockState::new(BlockKind::HoneyBlock),
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
    for _ in 0..3 {
        session.step().await.expect("piston motion should advance");
    }

    assert_eq!(session.world().state(slime.offset(Direction::East)).kind, BlockKind::SlimeBlock);
    assert_eq!(session.world().state(solid.offset(Direction::East)).kind, BlockKind::Solid);
    assert_eq!(session.world().state(honey).kind, BlockKind::HoneyBlock);
}

#[tokio::test]
async fn sticky_piston_finishes_in_flight_extension_before_fast_retraction() {
    let piston = Position::new(0, 0, 0);
    let block = Position::new(1, 0, 0);
    let source = Position::new(0, -1, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::StickyPiston).with_facing(Direction::East),
            },
            StructureBlock {
                position: block,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("piston should start extending");
    assert_eq!(session.world().state(block.offset(Direction::East)).kind, BlockKind::MovingPiston);
    session
        .apply(InputAction {
            tick: 2,
            operation: InputOperation::SetBlock {
                position: source,
                state: BlockState::new(BlockKind::Air),
            },
        })
        .await;
    session.step().await.expect("piston should retract during motion");

    assert!(!session.world().state(piston).extended());
    assert_eq!(session.world().state(block.offset(Direction::East)).kind, BlockKind::Solid);
}

#[tokio::test]
async fn sticky_piston_pulls_completed_extension_back_one_block() {
    let piston = Position::new(0, 0, 0);
    let block = Position::new(1, 0, 0);
    let source = Position::new(0, -1, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::StickyPiston).with_facing(Direction::East),
            },
            StructureBlock {
                position: block,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("piston should start extending");
    for _ in 0..3 {
        session.step().await.expect("piston motion should complete");
    }
    assert_eq!(session.world().state(block.offset(Direction::East)).kind, BlockKind::Solid);

    session
        .apply(InputAction {
            tick: 5,
            operation: InputOperation::SetBlock {
                position: source,
                state: BlockState::new(BlockKind::Air),
            },
        })
        .await;
    session.step().await.expect("sticky piston should retract");

    assert!(!session.world().state(piston).extended());
    assert_eq!(session.world().state(block).kind, BlockKind::MovingPiston);
    for _ in 0..3 {
        session.step().await.expect("retraction motion should complete");
    }
    assert_eq!(session.world().state(block).kind, BlockKind::Solid);
}

#[tokio::test]
async fn piston_refuses_to_push_more_than_twelve_blocks() {
    let piston = Position::new(0, 0, 0);
    let mut blocks = vec![
        StructureBlock {
            position: piston,
            state: BlockState::new(BlockKind::Piston).with_facing(Direction::East),
        },
        StructureBlock {
            position: Position::new(0, -1, 0),
            state: BlockState::new(BlockKind::RedstoneBlock),
        },
    ];
    for x in 1..=13 {
        blocks.push(StructureBlock {
            position: Position::new(x, 0, 0),
            state: BlockState::new(BlockKind::Solid),
        });
    }
    let mut session = SimulationSession::load_structure(StructureInput { blocks })
        .await
        .expect("structure should load");

    session.step().await.expect("piston should evaluate push limit");

    assert!(!session.world().state(piston).extended());
    assert_eq!(session.world().state(Position::new(13, 0, 0)).kind, BlockKind::Solid);
}

#[tokio::test]
async fn piston_destroys_redstone_wire_in_its_path() {
    let piston = Position::new(0, 0, 0);
    let wire = Position::new(1, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::Piston).with_facing(Direction::East),
            },
            StructureBlock {
                position: wire,
                state: BlockState::new(BlockKind::RedstoneWire),
            },
            StructureBlock {
                position: Position::new(0, -1, 0),
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
        ],
    })
    .await
    .expect("structure should load");

    session.step().await.expect("piston should extend");

    assert_eq!(session.world().state(wire).kind, BlockKind::MovingPiston);
    for _ in 0..3 {
        session.step().await.expect("piston motion should complete");
    }
    assert_eq!(session.world().state(wire).kind, BlockKind::PistonHead);
}

#[tokio::test]
async fn powered_conductor_supplies_adjacent_piston() {
    let source = Position::new(-2, 0, 0);
    let conductor = Position::new(-1, 0, 0);
    let piston = Position::new(0, 0, 0);
    let mut session = SimulationSession::load_structure(StructureInput {
        blocks: vec![
            StructureBlock {
                position: source,
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            StructureBlock {
                position: conductor,
                state: BlockState::new(BlockKind::Solid),
            },
            StructureBlock {
                position: piston,
                state: BlockState::new(BlockKind::Piston).with_facing(Direction::East),
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
    session.step().await.expect("piston should observe conductor power");

    assert!(session.world().state(piston).extended());
}
