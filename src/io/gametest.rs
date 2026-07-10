use std::collections::BTreeMap;
use std::path::Path;
use std::io::Write;

use fastnbt::to_bytes;
use flate2::{write::GzEncoder, Compression};
use serde::Serialize;
use thiserror::Error;

use crate::core::{BlockKind, BlockState, Direction, Position};
use crate::io::{StructureInput, TestVector};

const PACK_FORMAT: u32 = 101;
const NAMESPACE: &str = "redstone_oracle";
const TEST_ID: &str = "smoke";
const TRACE_TEST_ID: &str = "trace";

#[derive(Debug, Error)]
pub enum GameTestError {
    #[error("failed to write GameTest fixture: {0}")]
    Write(#[from] std::io::Error),
    #[error("failed to encode GameTest structure: {0}")]
    Nbt(String),
    #[error("cannot export block state to GameTest structure: {0:?}")]
    UnsupportedBlock(BlockKind),
    #[error("GameTest structure has no blocks")]
    EmptyStructure,
    #[error(transparent)]
    Structure(#[from] crate::io::StructureError),
}

#[derive(Serialize)]
struct StructureTemplate {
    #[serde(rename = "DataVersion")]
    data_version: i32,
    size: Vec<i32>,
    palette: Vec<PaletteEntry>,
    blocks: Vec<StructureBlock>,
    entities: Vec<StructureEntity>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct PaletteEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: std::collections::BTreeMap<String, String>,
}

#[derive(Serialize)]
struct StructureBlock {
    pos: Vec<i32>,
    state: i32,
}

#[derive(Serialize)]
struct StructureEntity {}

pub async fn write_smoke_datapack(root: impl AsRef<Path>) -> Result<(), GameTestError> {
    let root = root.as_ref();
    let data_root = root.join("data").join(NAMESPACE);
    tokio::fs::create_dir_all(data_root.join("test_instance")).await?;
    tokio::fs::create_dir_all(data_root.join("structure")).await?;

    let metadata = serde_json::json!({
        "pack": {
            "pack_format": PACK_FORMAT,
            "min_format": PACK_FORMAT,
            "max_format": PACK_FORMAT,
            "description": "redstone-rs GameTest oracle"
        }
    });
    let instance = serde_json::json!({
        "type": "minecraft:block_based",
        "environment": "minecraft:default",
        "structure": format!("{NAMESPACE}:{TEST_ID}"),
        "max_ticks": 20,
        "setup_ticks": 1,
        "required": true
    });
    let structure = StructureTemplate {
        data_version: 4790,
        size: vec![2, 1, 1],
        palette: vec![
            test_block("start"),
            test_block("accept"),
        ],
        blocks: vec![
            StructureBlock {
                pos: vec![0, 0, 0],
                state: 0,
            },
            StructureBlock {
                pos: vec![1, 0, 0],
                state: 1,
            },
        ],
        entities: Vec::new(),
    };
    let structure = encode_structure(&structure)?;

    tokio::fs::write(root.join("pack.mcmeta"), serde_json::to_vec_pretty(&metadata).expect("metadata is serializable")).await?;
    tokio::fs::write(
        data_root.join("test_instance").join(format!("{TEST_ID}.json")),
        serde_json::to_vec_pretty(&instance).expect("test instance is serializable"),
    )
    .await?;
    tokio::fs::write(data_root.join("structure").join(format!("{TEST_ID}.nbt")), structure).await?;
    Ok(())
}

pub async fn write_structure_template(
    path: impl AsRef<Path>,
    input: &StructureInput,
) -> Result<(), GameTestError> {
    let template = template_from_input(input)?;
    tokio::fs::write(path, encode_structure(&template)?).await?;
    Ok(())
}

pub async fn write_oracle_case_datapack(
    root: impl AsRef<Path>,
    vector: &TestVector,
    vector_root: impl AsRef<Path>,
) -> Result<(), GameTestError> {
    let root = root.as_ref();
    let data_root = root.join("data").join(NAMESPACE);
    tokio::fs::create_dir_all(data_root.join("test_instance")).await?;
    tokio::fs::create_dir_all(data_root.join("structure")).await?;

    let input = StructureInput::from_path(vector_root.as_ref().join(&vector.structure)).await?;
    write_structure_template(data_root.join("structure").join(format!("{TRACE_TEST_ID}.nbt")), &input).await?;

    let metadata = serde_json::json!({
        "pack": {
            "pack_format": PACK_FORMAT,
            "min_format": PACK_FORMAT,
            "max_format": PACK_FORMAT,
            "description": "redstone-rs GameTest oracle case"
        }
    });
    let instance = serde_json::json!({
        "type": "minecraft:function",
        "environment": "minecraft:default",
        "structure": format!("{NAMESPACE}:{TRACE_TEST_ID}"),
        "function": format!("{NAMESPACE}:{TRACE_TEST_ID}"),
        "max_ticks": vector.ticks.saturating_add(3),
        "setup_ticks": 1,
        "required": true
    });
    let config = serde_json::json!({
        "observe_min": vector.observe_min,
        "observe_max": vector.observe_max,
        "ticks": vector.ticks,
        "actions": vector.actions,
    });

    tokio::fs::write(root.join("pack.mcmeta"), serde_json::to_vec_pretty(&metadata).expect("metadata is serializable")).await?;
    tokio::fs::write(
        data_root.join("test_instance").join(format!("{TRACE_TEST_ID}.json")),
        serde_json::to_vec_pretty(&instance).expect("instance is serializable"),
    )
    .await?;
    tokio::fs::write(root.join("oracle-case.json"), serde_json::to_vec_pretty(&config).expect("case is serializable")).await?;
    Ok(())
}

fn encode_structure(template: &StructureTemplate) -> Result<Vec<u8>, GameTestError> {
    let encoded = to_bytes(template).map_err(|error| GameTestError::Nbt(error.to_string()))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&encoded)?;
    encoder.finish().map_err(GameTestError::Write)
}

fn template_from_input(input: &StructureInput) -> Result<StructureTemplate, GameTestError> {
    let min = input
        .blocks
        .iter()
        .map(|block| block.position)
        .reduce(min_position)
        .ok_or(GameTestError::EmptyStructure)?;
    let max = input.blocks.iter().map(|block| block.position).reduce(max_position).expect("input was non-empty");
    let mut palette = Vec::new();
    let mut palette_indexes = BTreeMap::new();
    let mut blocks = Vec::with_capacity(input.blocks.len());
    for block in &input.blocks {
        let entry = palette_entry(block.state)?;
        let next_index = palette.len() as i32;
        let state = *palette_indexes.entry(entry.clone()).or_insert_with(|| {
            palette.push(entry);
            next_index
        });
        blocks.push(StructureBlock {
            pos: vec![
                block.position.x - min.x,
                block.position.y - min.y,
                block.position.z - min.z,
            ],
            state,
        });
    }
    Ok(StructureTemplate {
        data_version: 4790,
        size: vec![max.x - min.x + 1, max.y - min.y + 1, max.z - min.z + 1],
        palette,
        blocks,
        entities: Vec::new(),
    })
}

fn palette_entry(state: BlockState) -> Result<PaletteEntry, GameTestError> {
    let mut properties = BTreeMap::new();
    let name = match state.kind {
        BlockKind::Air => "minecraft:air",
        BlockKind::Solid => "minecraft:stone",
        BlockKind::Glass => "minecraft:glass",
        BlockKind::Immovable => "minecraft:obsidian",
        BlockKind::RedstoneBlock => "minecraft:redstone_block",
        BlockKind::RedstoneWire => {
            properties.insert("power".to_owned(), state.power().to_string());
            "minecraft:redstone_wire"
        }
        BlockKind::RedstoneTorch => {
            if state.support_direction() == Direction::Down {
                "minecraft:redstone_torch"
            } else {
                properties.insert("facing".to_owned(), direction_name(state.facing()).to_owned());
                "minecraft:redstone_wall_torch"
            }
        }
        BlockKind::Lever => "minecraft:lever",
        BlockKind::Button if state.button_ticks() == 30 => "minecraft:oak_button",
        BlockKind::Button => "minecraft:stone_button",
        BlockKind::PressurePlate if state.analog_output() => "minecraft:light_weighted_pressure_plate",
        BlockKind::PressurePlate => "minecraft:stone_pressure_plate",
        BlockKind::Repeater => {
            properties.insert("delay".to_owned(), state.delay().to_string());
            properties.insert("locked".to_owned(), state.locked().to_string());
            "minecraft:repeater"
        }
        BlockKind::Comparator => {
            properties.insert("mode".to_owned(), if matches!(state.mode(), crate::core::ComparatorMode::Subtract) { "subtract" } else { "compare" }.to_owned());
            "minecraft:comparator"
        }
        BlockKind::Observer => "minecraft:observer",
        BlockKind::Lamp => "minecraft:redstone_lamp",
        BlockKind::CopperBulb => "minecraft:copper_bulb",
        BlockKind::DaylightDetector => "minecraft:daylight_detector",
        BlockKind::Target => "minecraft:target",
        BlockKind::Door => "minecraft:oak_door",
        BlockKind::Trapdoor => "minecraft:oak_trapdoor",
        BlockKind::FenceGate => "minecraft:oak_fence_gate",
        BlockKind::NoteBlock => "minecraft:note_block",
        BlockKind::Dropper => "minecraft:dropper",
        BlockKind::Dispenser => "minecraft:dispenser",
        BlockKind::Crafter => "minecraft:crafter",
        BlockKind::Tnt => "minecraft:tnt",
        BlockKind::Hopper => "minecraft:hopper",
        BlockKind::Container => "minecraft:barrel",
        BlockKind::Piston => "minecraft:piston",
        BlockKind::StickyPiston => "minecraft:sticky_piston",
        BlockKind::PistonHead => "minecraft:piston_head",
        BlockKind::SlimeBlock => "minecraft:slime_block",
        BlockKind::HoneyBlock => "minecraft:honey_block",
        BlockKind::MovingPiston => return Err(GameTestError::UnsupportedBlock(state.kind)),
    };
    if needs_facing(state.kind) {
        let facing = if matches!(state.kind, BlockKind::Repeater | BlockKind::Comparator) {
            state.facing().opposite()
        } else {
            state.facing()
        };
        properties.insert("facing".to_owned(), direction_name(facing).to_owned());
    }
    if matches!(state.kind, BlockKind::Lever | BlockKind::Button | BlockKind::PressurePlate | BlockKind::Repeater | BlockKind::Comparator | BlockKind::Observer | BlockKind::CopperBulb | BlockKind::Door | BlockKind::Trapdoor | BlockKind::FenceGate) {
        properties.insert("powered".to_owned(), state.powered().to_string());
    }
    if matches!(state.kind, BlockKind::Door | BlockKind::Trapdoor | BlockKind::FenceGate) {
        properties.insert("open".to_owned(), state.open().to_string());
    }
    if matches!(state.kind, BlockKind::Lamp | BlockKind::CopperBulb) {
        properties.insert("lit".to_owned(), state.lit().to_string());
    }
    if matches!(state.kind, BlockKind::Piston | BlockKind::StickyPiston) {
        properties.insert("extended".to_owned(), state.extended().to_string());
    }
    Ok(PaletteEntry {
        name: name.to_owned(),
        properties,
    })
}

fn needs_facing(kind: BlockKind) -> bool {
    matches!(
        kind,
        BlockKind::Lever
            | BlockKind::Button
            | BlockKind::Repeater
            | BlockKind::Comparator
            | BlockKind::Observer
            | BlockKind::Dropper
            | BlockKind::Dispenser
            | BlockKind::Crafter
            | BlockKind::Hopper
            | BlockKind::Piston
            | BlockKind::StickyPiston
            | BlockKind::PistonHead
    )
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::North => "north",
        Direction::South => "south",
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
    }
}

fn min_position(left: Position, right: Position) -> Position {
    Position::new(left.x.min(right.x), left.y.min(right.y), left.z.min(right.z))
}

fn max_position(left: Position, right: Position) -> Position {
    Position::new(left.x.max(right.x), left.y.max(right.y), left.z.max(right.z))
}

fn test_block(mode: &str) -> PaletteEntry {
    PaletteEntry {
        name: "minecraft:test_block".to_owned(),
        properties: std::collections::BTreeMap::from([("mode".to_owned(), mode.to_owned())]),
    }
}
