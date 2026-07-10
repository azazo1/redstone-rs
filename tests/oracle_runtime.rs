use std::{path::PathBuf, process::Command};

use redstone_rs::{
    api::InputOperation,
    core::{BlockKind, BlockState, Position},
    io::{write_structure_template, SimulationDifference, StructureBlock, StructureInput, TestVector},
    InputAction,
};

#[tokio::test]
#[ignore = "requires the Java 25 runtime and assets/server-26.1.2.jar"]
async fn java_oracle_matches_a_stable_redstone_block_vector() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = project_root
        .join(".gametest")
        .join(format!("oracle-runtime-{}", std::process::id()));
    let structure_path = fixture_root.join("machine.nbt");
    let vector_path = fixture_root.join("case.json");
    let output = fixture_root.join("output");
    tokio::fs::create_dir_all(&fixture_root)
        .await
        .expect("oracle fixture directory should create");
    write_structure_template(
        &structure_path,
        &StructureInput {
            blocks: vec![StructureBlock {
                position: Position::new(0, 0, 0),
                state: BlockState::new(BlockKind::RedstoneBlock),
            }],
        },
    )
    .await
    .expect("oracle structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: Vec::new(),
        observe_min: Position::new(0, 0, 0),
        observe_max: Position::new(0, 0, 0),
        ticks: 1,
    };
    tokio::fs::write(&vector_path, serde_json::to_vec(&vector).expect("vector should serialize"))
        .await
        .expect("oracle vector should write");

    let status = Command::new("just")
        .args(["oracle-run", vector_path.to_str().expect("vector path should be UTF-8"), "26.1.2", output.to_str().expect("output path should be UTF-8")])
        .current_dir(&project_root)
        .status()
        .expect("oracle recipe should start");
    assert!(status.success(), "oracle recipe should succeed");

    let differences: SimulationDifference = serde_json::from_slice(
        &tokio::fs::read(output.join("differences.json"))
            .await
            .expect("oracle differences should exist"),
    )
    .expect("oracle differences should be JSON");
    assert!(differences.is_empty(), "stable vector should match the Java oracle: {differences:?}");
}

#[tokio::test]
#[ignore = "requires the Java 25 runtime and assets/server-26.1.2.jar"]
async fn java_oracle_matches_lever_to_wire_timing() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = project_root
        .join(".gametest")
        .join(format!("oracle-lever-wire-{}", std::process::id()));
    let structure_path = fixture_root.join("machine.nbt");
    let vector_path = fixture_root.join("case.json");
    let output = fixture_root.join("output");
    tokio::fs::create_dir_all(&fixture_root)
        .await
        .expect("oracle fixture directory should create");
    write_structure_template(
        &structure_path,
        &StructureInput {
            blocks: vec![
                StructureBlock {
                    position: Position::new(0, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(1, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 1, 0),
                    state: BlockState::new(BlockKind::Lever),
                },
                StructureBlock {
                    position: Position::new(1, 1, 0),
                    state: BlockState::new(BlockKind::RedstoneWire),
                },
            ],
        },
    )
    .await
    .expect("oracle structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: vec![redstone_rs::io::TimedAction {
            action: InputAction {
                tick: 1,
                operation: InputOperation::UseBlock {
                    position: Position::new(0, 1, 0),
                },
            },
        }],
        observe_min: Position::new(1, 1, 0),
        observe_max: Position::new(1, 1, 0),
        ticks: 2,
    };
    tokio::fs::write(&vector_path, serde_json::to_vec(&vector).expect("vector should serialize"))
        .await
        .expect("oracle vector should write");

    let status = Command::new("just")
        .args(["oracle-run", vector_path.to_str().expect("vector path should be UTF-8"), "26.1.2", output.to_str().expect("output path should be UTF-8")])
        .current_dir(&project_root)
        .status()
        .expect("oracle recipe should start");
    assert!(status.success(), "oracle recipe should succeed");

    let differences: SimulationDifference = serde_json::from_slice(
        &tokio::fs::read(output.join("differences.json"))
            .await
            .expect("oracle differences should exist"),
    )
    .expect("oracle differences should be JSON");
    assert!(differences.is_empty(), "lever to wire timing should match the Java oracle: {differences:?}");
}

#[tokio::test]
#[ignore = "requires the Java 25 runtime and assets/server-26.1.2.jar"]
async fn java_oracle_matches_repeater_delay_timing() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = project_root
        .join(".gametest")
        .join(format!("oracle-repeater-{}", std::process::id()));
    let structure_path = fixture_root.join("machine.nbt");
    let vector_path = fixture_root.join("case.json");
    let output = fixture_root.join("output");
    tokio::fs::create_dir_all(&fixture_root)
        .await
        .expect("oracle fixture directory should create");
    write_structure_template(
        &structure_path,
        &StructureInput {
            blocks: vec![
                StructureBlock {
                    position: Position::new(0, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 0, 1),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 0, 2),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(1, 0, 2),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 1, 0),
                    state: BlockState::new(BlockKind::RedstoneWire),
                },
                StructureBlock {
                    position: Position::new(0, 1, 1),
                    state: BlockState::new(BlockKind::Repeater),
                },
                StructureBlock {
                    position: Position::new(0, 1, 2),
                    state: BlockState::new(BlockKind::RedstoneWire),
                },
                StructureBlock {
                    position: Position::new(1, 1, 2),
                    state: BlockState::new(BlockKind::Lever),
                },
            ],
        },
    )
    .await
    .expect("oracle structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: vec![redstone_rs::io::TimedAction {
            action: InputAction {
                tick: 1,
                operation: InputOperation::UseBlock {
                    position: Position::new(1, 1, 2),
                },
            },
        }],
        observe_min: Position::new(0, 1, 0),
        observe_max: Position::new(0, 1, 0),
        ticks: 5,
    };
    tokio::fs::write(&vector_path, serde_json::to_vec(&vector).expect("vector should serialize"))
        .await
        .expect("oracle vector should write");

    let status = Command::new("just")
        .args(["oracle-run", vector_path.to_str().expect("vector path should be UTF-8"), "26.1.2", output.to_str().expect("output path should be UTF-8")])
        .current_dir(&project_root)
        .status()
        .expect("oracle recipe should start");
    assert!(status.success(), "oracle recipe should succeed");

    let differences: SimulationDifference = serde_json::from_slice(
        &tokio::fs::read(output.join("differences.json"))
            .await
            .expect("oracle differences should exist"),
    )
    .expect("oracle differences should be JSON");
    assert!(differences.is_empty(), "repeater delay timing should match the Java oracle: {differences:?}");
}

#[tokio::test]
#[ignore = "requires the Java 25 runtime and assets/server-26.1.2.jar"]
async fn java_oracle_matches_comparator_timing() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = project_root
        .join(".gametest")
        .join(format!("oracle-comparator-{}", std::process::id()));
    let structure_path = fixture_root.join("machine.nbt");
    let vector_path = fixture_root.join("case.json");
    let output = fixture_root.join("output");
    tokio::fs::create_dir_all(&fixture_root)
        .await
        .expect("oracle fixture directory should create");
    write_structure_template(
        &structure_path,
        &StructureInput {
            blocks: vec![
                StructureBlock {
                    position: Position::new(0, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 0, 1),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 0, 2),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(1, 0, 2),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 1, 0),
                    state: BlockState::new(BlockKind::RedstoneWire),
                },
                StructureBlock {
                    position: Position::new(0, 1, 1),
                    state: BlockState::new(BlockKind::Comparator),
                },
                StructureBlock {
                    position: Position::new(0, 1, 2),
                    state: BlockState::new(BlockKind::RedstoneWire),
                },
                StructureBlock {
                    position: Position::new(1, 1, 2),
                    state: BlockState::new(BlockKind::Lever),
                },
            ],
        },
    )
    .await
    .expect("oracle structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: vec![redstone_rs::io::TimedAction {
            action: InputAction {
                tick: 1,
                operation: InputOperation::UseBlock {
                    position: Position::new(1, 1, 2),
                },
            },
        }],
        observe_min: Position::new(0, 1, 0),
        observe_max: Position::new(0, 1, 0),
        ticks: 4,
    };
    tokio::fs::write(&vector_path, serde_json::to_vec(&vector).expect("vector should serialize"))
        .await
        .expect("oracle vector should write");

    let status = Command::new("just")
        .args(["oracle-run", vector_path.to_str().expect("vector path should be UTF-8"), "26.1.2", output.to_str().expect("output path should be UTF-8")])
        .current_dir(&project_root)
        .status()
        .expect("oracle recipe should start");
    assert!(status.success(), "oracle recipe should succeed");

    let differences: SimulationDifference = serde_json::from_slice(
        &tokio::fs::read(output.join("differences.json"))
            .await
            .expect("oracle differences should exist"),
    )
    .expect("oracle differences should be JSON");
    assert!(differences.is_empty(), "comparator timing should match the Java oracle: {differences:?}");
}

#[tokio::test]
#[ignore = "requires the Java 25 runtime and assets/server-26.1.2.jar"]
async fn java_oracle_matches_piston_extension() {
    let project_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_root = project_root
        .join(".gametest")
        .join(format!("oracle-piston-{}", std::process::id()));
    let structure_path = fixture_root.join("machine.nbt");
    let vector_path = fixture_root.join("case.json");
    let output = fixture_root.join("output");
    tokio::fs::create_dir_all(&fixture_root)
        .await
        .expect("oracle fixture directory should create");
    write_structure_template(
        &structure_path,
        &StructureInput {
            blocks: vec![
                StructureBlock {
                    position: Position::new(0, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(1, 0, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(0, 1, 0),
                    state: BlockState::new(BlockKind::Piston).with_facing(redstone_rs::Direction::East),
                },
                StructureBlock {
                    position: Position::new(1, 1, 0),
                    state: BlockState::new(BlockKind::Solid),
                },
                StructureBlock {
                    position: Position::new(2, 1, 0),
                    state: BlockState::new(BlockKind::Air),
                },
                StructureBlock {
                    position: Position::new(0, 1, -1),
                    state: BlockState::new(BlockKind::Air),
                },
            ],
        },
    )
    .await
    .expect("oracle structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: vec![redstone_rs::io::TimedAction {
            action: InputAction {
                tick: 1,
                operation: InputOperation::SetBlock {
                    position: Position::new(0, 1, -1),
                    state: BlockState::new(BlockKind::RedstoneBlock),
                },
            },
        }],
        observe_min: Position::new(0, 1, 0),
        observe_max: Position::new(2, 1, 0),
        ticks: 5,
    };
    tokio::fs::write(&vector_path, serde_json::to_vec(&vector).expect("vector should serialize"))
        .await
        .expect("oracle vector should write");

    let status = Command::new("just")
        .args(["oracle-run", vector_path.to_str().expect("vector path should be UTF-8"), "26.1.2", output.to_str().expect("output path should be UTF-8")])
        .current_dir(&project_root)
        .status()
        .expect("oracle recipe should start");
    assert!(status.success(), "oracle recipe should succeed");

    let differences: SimulationDifference = serde_json::from_slice(
        &tokio::fs::read(output.join("differences.json"))
            .await
            .expect("oracle differences should exist"),
    )
    .expect("oracle differences should be JSON");
    assert!(differences.is_empty(), "piston extension should match the Java oracle: {differences:?}");
}
