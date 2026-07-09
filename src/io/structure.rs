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
    } else if name.ends_with("_button") {
        BlockKind::Button
    } else if is_supported_solid(name) {
        BlockKind::Solid
    } else if is_supported_non_conductor(name) {
        BlockKind::Glass
    } else if is_supported_immovable(name) {
        BlockKind::Immovable
    } else {
        match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => BlockKind::Air,
        "minecraft:redstone_block" => BlockKind::RedstoneBlock,
        "minecraft:redstone_wire" => BlockKind::RedstoneWire,
        "minecraft:redstone_torch" | "minecraft:redstone_wall_torch" => BlockKind::RedstoneTorch,
        "minecraft:lever" => BlockKind::Lever,
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
    if matches!(name, "minecraft:light_weighted_pressure_plate" | "minecraft:heavy_weighted_pressure_plate") {
        state = state.with_analog_output(true);
    }
    if name == "minecraft:redstone_wall_torch" {
        state = state.with_wall_mounted(true);
    }
    if kind == BlockKind::Button && is_wooden_button(name) {
        state = state.with_wooden_button(true);
    }
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
        state = state.with_open(parse_bool(open)?);
    }
    Ok(state)
}

fn is_supported_solid(name: &str) -> bool {
    matches!(
        name,
        "minecraft:stone"
            | "minecraft:granite"
            | "minecraft:diorite"
            | "minecraft:andesite"
            | "minecraft:deepslate"
            | "minecraft:cobbled_deepslate"
            | "minecraft:polished_deepslate"
            | "minecraft:deepslate_bricks"
            | "minecraft:deepslate_tiles"
            | "minecraft:tuff"
            | "minecraft:calcite"
            | "minecraft:dripstone_block"
            | "minecraft:cobblestone"
            | "minecraft:mossy_cobblestone"
            | "minecraft:stone_bricks"
            | "minecraft:mossy_stone_bricks"
            | "minecraft:cracked_stone_bricks"
            | "minecraft:chiseled_stone_bricks"
            | "minecraft:polished_blackstone"
            | "minecraft:polished_blackstone_bricks"
            | "minecraft:bricks"
            | "minecraft:nether_bricks"
            | "minecraft:red_nether_bricks"
            | "minecraft:end_stone"
            | "minecraft:end_stone_bricks"
            | "minecraft:netherrack"
            | "minecraft:basalt"
            | "minecraft:smooth_basalt"
            | "minecraft:blackstone"
            | "minecraft:quartz_block"
            | "minecraft:quartz_bricks"
            | "minecraft:purpur_block"
            | "minecraft:purpur_pillar"
            | "minecraft:sandstone"
            | "minecraft:cut_sandstone"
            | "minecraft:smooth_sandstone"
            | "minecraft:red_sandstone"
            | "minecraft:cut_red_sandstone"
            | "minecraft:smooth_red_sandstone"
            | "minecraft:dirt"
            | "minecraft:coarse_dirt"
            | "minecraft:rooted_dirt"
            | "minecraft:grass_block"
            | "minecraft:podzol"
            | "minecraft:mycelium"
            | "minecraft:clay"
            | "minecraft:packed_mud"
            | "minecraft:mud_bricks"
            | "minecraft:bone_block"
            | "minecraft:snow_block"
            | "minecraft:hay_block"
            | "minecraft:bamboo_block"
            | "minecraft:iron_block"
            | "minecraft:gold_block"
            | "minecraft:diamond_block"
            | "minecraft:emerald_block"
            | "minecraft:lapis_block"
            | "minecraft:coal_block"
            | "minecraft:raw_iron_block"
            | "minecraft:raw_gold_block"
            | "minecraft:raw_copper_block"
            | "minecraft:copper_block"
            | "minecraft:exposed_copper"
            | "minecraft:weathered_copper"
            | "minecraft:oxidized_copper"
            | "minecraft:terracotta"
            | "minecraft:white_terracotta"
            | "minecraft:orange_terracotta"
            | "minecraft:magenta_terracotta"
            | "minecraft:light_blue_terracotta"
            | "minecraft:yellow_terracotta"
            | "minecraft:lime_terracotta"
            | "minecraft:pink_terracotta"
            | "minecraft:gray_terracotta"
            | "minecraft:light_gray_terracotta"
            | "minecraft:cyan_terracotta"
            | "minecraft:purple_terracotta"
            | "minecraft:blue_terracotta"
            | "minecraft:brown_terracotta"
            | "minecraft:green_terracotta"
            | "minecraft:red_terracotta"
            | "minecraft:black_terracotta"
    ) || name.ends_with("_planks")
        || name.ends_with("_log")
        || name.ends_with("_wood")
        || name.ends_with("_stem")
        || name.ends_with("_hyphae")
        || name.ends_with("_wool")
        || name.ends_with("_concrete")
        || name.ends_with("_concrete_powder")
}

fn is_supported_non_conductor(name: &str) -> bool {
    matches!(name, "minecraft:glass" | "minecraft:glass_pane" | "minecraft:tinted_glass")
        || name.ends_with("_stained_glass")
        || name.ends_with("_stained_glass_pane")
}

fn is_supported_immovable(name: &str) -> bool {
    matches!(
        name,
        "minecraft:obsidian"
            | "minecraft:crying_obsidian"
            | "minecraft:reinforced_deepslate"
            | "minecraft:bedrock"
            | "minecraft:barrier"
            | "minecraft:end_portal_frame"
            | "minecraft:command_block"
            | "minecraft:chain_command_block"
            | "minecraft:repeating_command_block"
            | "minecraft:structure_block"
            | "minecraft:jigsaw"
            | "minecraft:spawner"
    )
}

fn is_wooden_button(name: &str) -> bool {
    matches!(
        name,
        "minecraft:oak_button"
            | "minecraft:spruce_button"
            | "minecraft:birch_button"
            | "minecraft:jungle_button"
            | "minecraft:acacia_button"
            | "minecraft:dark_oak_button"
            | "minecraft:mangrove_button"
            | "minecraft:cherry_button"
            | "minecraft:pale_oak_button"
            | "minecraft:bamboo_button"
            | "minecraft:crimson_button"
            | "minecraft:warped_button"
    )
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
