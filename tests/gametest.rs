use std::collections::BTreeMap;

use fastnbt::from_bytes;
use flate2::read::GzDecoder;
use std::io::Read;
use redstone_rs::{
    core::{BlockKind, BlockState, Position},
    io::{write_oracle_case_datapack, write_smoke_datapack, write_structure_template, StructureBlock as SimStructureBlock, StructureInput, TestVector},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct StructureTemplate {
    #[serde(rename = "DataVersion")]
    data_version: i32,
    size: Vec<i32>,
    palette: Vec<PaletteEntry>,
    blocks: Vec<StructureBlock>,
}

#[derive(Deserialize)]
struct PaletteEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct StructureBlock {
    pos: Vec<i32>,
    state: i32,
}

#[tokio::test]
async fn writes_block_based_smoke_datapack() {
    let root = std::env::temp_dir().join(format!("redstone-rs-gametest-{}", std::process::id()));
    write_smoke_datapack(&root).await.expect("datapack should write");

    let instance = tokio::fs::read(root.join("data/redstone_oracle/test_instance/smoke.json"))
        .await
        .expect("test instance should exist");
    let instance: serde_json::Value = serde_json::from_slice(&instance).expect("test instance should be JSON");
    assert_eq!(instance["type"], "minecraft:block_based");
    assert_eq!(instance["structure"], "redstone_oracle:smoke");

    let structure = tokio::fs::read(root.join("data/redstone_oracle/structure/smoke.nbt"))
        .await
        .expect("structure should exist");
    let mut structure = GzDecoder::new(structure.as_slice());
    let mut decoded = Vec::new();
    structure.read_to_end(&mut decoded).expect("structure should decompress");
    let structure: StructureTemplate = from_bytes(&decoded).expect("structure should be NBT");
    std::fs::remove_dir_all(&root).expect("fixture directory should remove");

    assert_eq!(structure.data_version, 4790);
    assert_eq!(structure.size, vec![2, 1, 1]);
    assert_eq!(structure.palette.len(), 2);
    assert_eq!(structure.palette[0].name, "minecraft:test_block");
    assert_eq!(structure.palette[0].properties["mode"], "start");
    assert_eq!(structure.palette[1].properties["mode"], "accept");
    assert_eq!(structure.blocks.len(), 2);
    assert_eq!(structure.blocks[1].pos, vec![1, 0, 0]);
    assert_eq!(structure.blocks[1].state, 1);
}

#[tokio::test]
async fn exports_simulator_blocks_as_gzipped_structure_template() {
    let root = std::env::temp_dir().join(format!("redstone-rs-structure-{}", std::process::id()));
    let path = root.join("machine.nbt");
    let input = StructureInput {
        blocks: vec![
            SimStructureBlock {
                position: Position::new(4, 8, 12),
                state: BlockState::new(BlockKind::RedstoneBlock),
            },
            SimStructureBlock {
                position: Position::new(5, 8, 12),
                state: BlockState::new(BlockKind::RedstoneWire).with_power(9),
            },
            SimStructureBlock {
                position: Position::new(6, 8, 12),
                state: BlockState::new(BlockKind::Button).with_wooden_button(true),
            },
        ],
    };
    tokio::fs::create_dir_all(&root).await.expect("fixture directory should create");
    write_structure_template(&path, &input).await.expect("structure should write");

    let structure = tokio::fs::read(&path).await.expect("structure should exist");
    let mut structure = GzDecoder::new(structure.as_slice());
    let mut decoded = Vec::new();
    structure.read_to_end(&mut decoded).expect("structure should decompress");
    let structure: StructureTemplate = from_bytes(&decoded).expect("structure should be NBT");
    std::fs::remove_dir_all(&root).expect("fixture directory should remove");

    assert_eq!(structure.size, vec![3, 1, 1]);
    assert_eq!(structure.blocks[0].pos, vec![0, 0, 0]);
    assert_eq!(structure.palette[0].name, "minecraft:redstone_block");
    assert_eq!(structure.palette[1].name, "minecraft:redstone_wire");
    assert_eq!(structure.palette[1].properties["power"], "9");
    assert_eq!(structure.palette[2].name, "minecraft:oak_button");
}

#[tokio::test]
async fn writes_function_based_oracle_case_datapack() {
    let root = std::env::temp_dir().join(format!("redstone-rs-oracle-case-{}", std::process::id()));
    let source = root.join("source");
    tokio::fs::create_dir_all(&source).await.expect("source should create");
    let input = StructureInput {
        blocks: vec![SimStructureBlock {
            position: Position::new(0, 0, 0),
            state: BlockState::new(BlockKind::RedstoneBlock),
        }],
    };
    write_structure_template(source.join("machine.nbt"), &input)
        .await
        .expect("source structure should write");
    let vector = TestVector {
        structure: "machine.nbt".to_owned(),
        actions: Vec::new(),
        observe_min: Position::new(0, 0, 0),
        observe_max: Position::new(0, 0, 0),
        ticks: 4,
    };

    write_oracle_case_datapack(root.join("pack"), &vector, &source)
        .await
        .expect("oracle case should write");

    let instance = tokio::fs::read(root.join("pack/data/redstone_oracle/test_instance/trace.json"))
        .await
        .expect("trace instance should exist");
    let instance: serde_json::Value = serde_json::from_slice(&instance).expect("instance should be JSON");
    let config = tokio::fs::read(root.join("pack/oracle-case.json"))
        .await
        .expect("oracle config should exist");
    let config: serde_json::Value = serde_json::from_slice(&config).expect("config should be JSON");
    std::fs::remove_dir_all(&root).expect("fixture directory should remove");

    assert_eq!(instance["type"], "minecraft:function");
    assert_eq!(instance["function"], "redstone_oracle:trace");
    assert_eq!(instance["max_ticks"], 7);
    assert_eq!(config["ticks"], 4);
}
