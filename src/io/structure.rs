use std::collections::BTreeMap;
use std::path::Path;

use fastnbt::from_bytes;
use serde::Deserialize;
use thiserror::Error;

use crate::core::{BlockKind, BlockState, ComparatorMode, Direction, Position};

#[derive(Clone, Debug)]
pub struct StructureBlock {
    pub position: Position,
    pub state: BlockState,
}

#[derive(Clone, Debug, Default)]
pub struct StructureInput {
    pub blocks: Vec<StructureBlock>,
}

#[derive(Debug, Error)]
pub enum StructureError {
    #[error("failed to read structure: {0}")]
    Read(#[from] std::io::Error),
    #[error("invalid structure nbt: {0}")]
    Nbt(String),
    #[error("structure block position is invalid")]
    InvalidPosition,
    #[error("unsupported block in exact mode: {0}")]
    UnsupportedBlock(String),
}

#[derive(Debug, Deserialize)]
struct RawStructure {
    palette: Vec<RawPaletteEntry>,
    blocks: Vec<RawBlock>,
}

#[derive(Debug, Deserialize)]
struct RawPaletteEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawBlock {
    pos: Vec<i32>,
    state: usize,
}

impl StructureInput {
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, StructureError> {
        let bytes = tokio::fs::read(path).await?;
        Self::from_nbt(&bytes)
    }

    pub fn from_nbt(bytes: &[u8]) -> Result<Self, StructureError> {
        let raw: RawStructure = from_bytes(bytes).map_err(|error| StructureError::Nbt(error.to_string()))?;
        let palette = raw
            .palette
            .iter()
            .map(state_from_palette)
            .collect::<Result<Vec<_>, _>>()?;
        let mut blocks = Vec::with_capacity(raw.blocks.len());
        for block in raw.blocks {
            let [x, y, z] = block.pos.as_slice() else {
                return Err(StructureError::InvalidPosition);
            };
            let state = palette.get(block.state).copied().ok_or(StructureError::InvalidPosition)?;
            blocks.push(StructureBlock {
                position: Position::new(*x, *y, *z),
                state,
            });
        }
        Ok(Self { blocks })
    }
}

fn state_from_palette(entry: &RawPaletteEntry) -> Result<BlockState, StructureError> {
    let kind = match entry.name.as_str() {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => BlockKind::Air,
        "minecraft:stone" | "minecraft:cobblestone" | "minecraft:dirt" | "minecraft:oak_planks" | "minecraft:iron_block" => BlockKind::Solid,
        "minecraft:glass" => BlockKind::Glass,
        "minecraft:obsidian" | "minecraft:crying_obsidian" | "minecraft:reinforced_deepslate" => BlockKind::Immovable,
        "minecraft:redstone_block" => BlockKind::RedstoneBlock,
        "minecraft:redstone_wire" => BlockKind::RedstoneWire,
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => BlockKind::RedstoneTorch,
        "minecraft:lever" => BlockKind::Lever,
        "minecraft:stone_button" | "minecraft:oak_button" | "minecraft:polished_blackstone_button" => BlockKind::Button,
        "minecraft:stone_pressure_plate" | "minecraft:light_weighted_pressure_plate" | "minecraft:heavy_weighted_pressure_plate" => BlockKind::PressurePlate,
        "minecraft:repeater" => BlockKind::Repeater,
        "minecraft:comparator" => BlockKind::Comparator,
        "minecraft:observer" => BlockKind::Observer,
        "minecraft:redstone_lamp" => BlockKind::Lamp,
        "minecraft:barrel" | "minecraft:chest" | "minecraft:trapped_chest" => BlockKind::Container,
        "minecraft:piston" => BlockKind::Piston,
        "minecraft:sticky_piston" => BlockKind::StickyPiston,
        "minecraft:piston_head" => BlockKind::PistonHead,
        "minecraft:moving_piston" => BlockKind::MovingPiston,
        "minecraft:slime_block" => BlockKind::SlimeBlock,
        "minecraft:honey_block" => BlockKind::HoneyBlock,
        _ => return Err(StructureError::UnsupportedBlock(entry.name.clone())),
    };

    let mut state = BlockState::new(kind);
    if let Some(facing) = entry.properties.get("facing") {
        state = state.with_facing(parse_direction(facing)?);
    }
    if let Some(powered) = entry.properties.get("powered") {
        state = state.with_powered(parse_bool(powered)?);
    }
    if let Some(power) = entry.properties.get("power") {
        state = state.with_power(power.parse::<u8>().map_err(|_| StructureError::UnsupportedBlock(entry.name.clone()))?);
    }
    if let Some(delay) = entry.properties.get("delay") {
        state = state.with_delay(delay.parse::<u8>().map_err(|_| StructureError::UnsupportedBlock(entry.name.clone()))?);
    }
    if let Some(mode) = entry.properties.get("mode") {
        state = state.with_mode(match mode.as_str() {
            "compare" => ComparatorMode::Compare,
            "subtract" => ComparatorMode::Subtract,
            _ => return Err(StructureError::UnsupportedBlock(entry.name.clone())),
        });
    }
    if let Some(extended) = entry.properties.get("extended") {
        state = state.with_extended(parse_bool(extended)?);
    }
    Ok(state)
}

fn parse_direction(value: &str) -> Result<Direction, StructureError> {
    match value {
        "north" => Ok(Direction::North),
        "south" => Ok(Direction::South),
        "west" => Ok(Direction::West),
        "east" => Ok(Direction::East),
        "down" => Ok(Direction::Down),
        "up" => Ok(Direction::Up),
        _ => Err(StructureError::UnsupportedBlock(value.to_owned())),
    }
}

fn parse_bool(value: &str) -> Result<bool, StructureError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(StructureError::UnsupportedBlock(value.to_owned())),
    }
}
