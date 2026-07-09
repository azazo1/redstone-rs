use std::collections::BTreeMap;

use fastnbt::from_bytes;
use flate2::read::GzDecoder;
use std::io::Read;
use redstone_rs::{
    core::{BlockKind, BlockState, Position},
    io::{write_smoke_datapack, write_structure_template, StructureBlock as SimStructureBlock, StructureInput},
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

    assert_eq!(structure.size, vec![2, 1, 1]);
    assert_eq!(structure.blocks[0].pos, vec![0, 0, 0]);
    assert_eq!(structure.palette[0].name, "minecraft:redstone_block");
    assert_eq!(structure.palette[1].name, "minecraft:redstone_wire");
    assert_eq!(structure.palette[1].properties["power"], "9");
}
