use std::path::Path;

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
    #[error("structure dimensions are invalid")]
    InvalidDimensions,
    #[error("structure block data is truncated")]
    TruncatedBlockData,
    #[error("unsupported block in exact mode: {0}")]
    UnsupportedBlock(String),
}

impl StructureInput {
    pub async fn from_path(path: impl AsRef<Path>) -> Result<Self, StructureError> {
        let path = path.as_ref();
        let bytes = tokio::fs::read(path).await?;
        if path.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("mca")) {
            return super::format::decode_anvil_region(&bytes);
        }
        Self::from_nbt(&bytes)
    }

    pub fn from_nbt(bytes: &[u8]) -> Result<Self, StructureError> {
        super::format::decode(bytes)
    }
}

pub(crate) fn state_from_parts(
    name: &str,
    properties: &std::collections::BTreeMap<String, String>,
) -> Result<BlockState, StructureError> {
    let kind = if name.ends_with("_trapdoor") {
        BlockKind::Trapdoor
    } else if name.ends_with("_door") {
        BlockKind::Door
    } else if name.ends_with("_fence_gate") {
        BlockKind::FenceGate
    } else {
        match name {
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
        "minecraft:copper_bulb" | "minecraft:exposed_copper_bulb" | "minecraft:weathered_copper_bulb" | "minecraft:oxidized_copper_bulb" | "minecraft:waxed_copper_bulb" => BlockKind::CopperBulb,
        "minecraft:daylight_detector" => BlockKind::DaylightDetector,
        "minecraft:target" => BlockKind::Target,
        "minecraft:note_block" => BlockKind::NoteBlock,
        "minecraft:dropper" => BlockKind::Dropper,
        "minecraft:dispenser" => BlockKind::Dispenser,
        "minecraft:crafter" => BlockKind::Crafter,
        "minecraft:tnt" => BlockKind::Tnt,
        "minecraft:hopper" => BlockKind::Hopper,
        "minecraft:barrel" | "minecraft:chest" | "minecraft:trapped_chest" => BlockKind::Container,
        "minecraft:piston" => BlockKind::Piston,
        "minecraft:sticky_piston" => BlockKind::StickyPiston,
        "minecraft:piston_head" => BlockKind::PistonHead,
        "minecraft:moving_piston" => BlockKind::MovingPiston,
        "minecraft:slime_block" => BlockKind::SlimeBlock,
        "minecraft:honey_block" => BlockKind::HoneyBlock,
        _ => return Err(StructureError::UnsupportedBlock(name.to_owned())),
        }
    };

    let mut state = BlockState::new(kind);
    if let Some(facing) = properties.get("facing") {
        state = state.with_facing(parse_direction(facing)?);
    }
    if let Some(powered) = properties.get("powered") {
        state = state.with_powered(parse_bool(powered)?);
    }
    if let Some(lit) = properties.get("lit") {
        let lit = parse_bool(lit)?;
        state = if kind == BlockKind::RedstoneTorch {
            state.with_powered(lit)
        } else {
            state.with_lit(lit)
        };
    }
    if let Some(power) = properties.get("power") {
        state = state.with_power(power.parse::<u8>().map_err(|_| StructureError::UnsupportedBlock(name.to_owned()))?);
    }
    if let Some(delay) = properties.get("delay") {
        state = state.with_delay(delay.parse::<u8>().map_err(|_| StructureError::UnsupportedBlock(name.to_owned()))?);
    }
    if let Some(mode) = properties.get("mode") {
        state = state.with_mode(match mode.as_str() {
            "compare" => ComparatorMode::Compare,
            "subtract" => ComparatorMode::Subtract,
            _ => return Err(StructureError::UnsupportedBlock(name.to_owned())),
        });
    }
    if let Some(extended) = properties.get("extended") {
        state = state.with_extended(parse_bool(extended)?);
    }
    if let Some(open) = properties.get("open") {
        state = state.with_powered(parse_bool(open)?);
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
