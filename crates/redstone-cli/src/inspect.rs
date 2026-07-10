use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{LoadedStructure, StructureLoader};
use redstone_java_26::{Java26Registry, StateDefinition};
use serde::Serialize;

use crate::{RegistryResolver, reject_newer_data_version};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

pub fn parse_block_pos(value: &str) -> Result<BlockPos, String> {
    let coordinates = value.split(',').map(str::trim).collect::<Vec<_>>();
    if coordinates.len() != 3 {
        return Err("坐标必须使用 X,Y,Z 格式".to_owned());
    }
    let parse = |coordinate: &str| {
        coordinate
            .parse::<i32>()
            .map_err(|_| format!("坐标值不是有效的 32 位整数: {coordinate}"))
    };
    Ok(BlockPos::new(
        parse(coordinates[0])?,
        parse(coordinates[1])?,
        parse(coordinates[2])?,
    ))
}

pub fn run(
    path: &Path,
    requested_positions: &[BlockPos],
    all: bool,
    requested_types: &[String],
    output_format: OutputFormat,
) -> Result<()> {
    let mut resolver = RegistryResolver(Java26Registry::new());
    let structure = StructureLoader::load(path, BlockPos::ZERO, &mut resolver)?;
    reject_newer_data_version(structure.data_version)?;

    let unsupported_active_blocks = resolver
        .0
        .states()
        .filter(|state| !state.supported)
        .map(|state| state.name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let positions = selected_positions(
        &structure,
        &resolver.0,
        requested_positions,
        all,
        requested_types,
    )?;
    let blocks = positions
        .into_iter()
        .map(|pos| inspect_block(&structure, &resolver.0, pos))
        .collect::<Result<Vec<_>>>()?;
    let report = InspectReport {
        format: &structure.format,
        data_version: structure.data_version,
        bounds: InspectBounds {
            min: structure.min,
            max: structure.max,
        },
        non_air_blocks: structure.world.non_air_blocks(),
        sections: structure.world.section_count(),
        block_types: &structure.block_counts,
        unsupported_active_blocks,
        blocks,
    };

    match output_format {
        OutputFormat::Text => write_text(&report),
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(())
        }
    }
}

fn selected_positions(
    structure: &LoadedStructure,
    registry: &Java26Registry,
    requested_positions: &[BlockPos],
    all: bool,
    requested_types: &[String],
) -> Result<Vec<BlockPos>> {
    if !requested_positions.is_empty() {
        return Ok(requested_positions.to_vec());
    }
    if !all && requested_types.is_empty() {
        return Ok(Vec::new());
    }

    let requested_types = requested_types
        .iter()
        .map(|name| {
            if name.contains(':') {
                name.clone()
            } else {
                format!("minecraft:{name}")
            }
        })
        .collect::<BTreeSet<_>>();
    let mut positions = structure
        .world
        .iter_blocks()
        .filter_map(|(pos, state_id)| {
            if all {
                return Some(pos);
            }
            registry
                .state(state_id)
                .is_some_and(|state| requested_types.contains(&state.name))
                .then_some(pos)
        })
        .collect::<Vec<_>>();
    positions.sort_unstable();
    Ok(positions)
}

fn inspect_block(
    structure: &LoadedStructure,
    registry: &Java26Registry,
    pos: BlockPos,
) -> Result<BlockInspection> {
    let state_id = structure.world.get_block(pos);
    let state = registry
        .state(state_id)
        .ok_or_else(|| anyhow!("找不到坐标 {pos:?} 的状态定义: {}", state_id.0))?;
    Ok(BlockInspection {
        position: pos,
        state: state_inspection(state),
        block_entity: structure.world.block_entity(pos).map(|data| {
            let nbt = structure
                .block_entity_nbt
                .get(&pos)
                .cloned()
                .unwrap_or_else(|| {
                    let mut nbt = data.fields.clone();
                    nbt.insert("id".to_owned(), serde_json::Value::String(data.kind.clone()));
                    nbt
                });
            BlockEntityInspection {
                id: data.kind.clone(),
                nbt,
            }
        }),
    })
}

fn state_inspection(state: &StateDefinition) -> BlockStateInspection {
    BlockStateInspection {
        id: state.id,
        name: state.name.clone(),
        properties: state.properties.clone(),
        supported: state.supported,
    }
}

fn write_text(report: &InspectReport<'_>) -> Result<()> {
    println!("format: {}", report.format);
    println!("data_version: {:?}", report.data_version);
    println!("bounds: {:?} .. {:?}", report.bounds.min, report.bounds.max);
    println!("non_air_blocks: {}", report.non_air_blocks);
    println!("sections: {}", report.sections);
    println!("block_types:");
    for (name, count) in report.block_types {
        println!("  {name}: {count}");
    }
    if !report.unsupported_active_blocks.is_empty() {
        println!("unsupported_active_blocks:");
        for name in &report.unsupported_active_blocks {
            println!("  {name}");
        }
    }
    if report.blocks.is_empty() {
        return Ok(());
    }

    println!("blocks:");
    for block in &report.blocks {
        println!(
            "  - position: [{}, {}, {}]",
            block.position.x, block.position.y, block.position.z
        );
        println!("    state_id: {}", block.state.id.0);
        println!("    name: {}", block.state.name);
        println!(
            "    properties: {}",
            serde_json::to_string(&block.state.properties)?
        );
        println!("    supported: {}", block.state.supported);
        if let Some(block_entity) = &block.block_entity {
            println!("    block_entity:");
            println!("      id: {}", block_entity.id);
            println!("      nbt:");
            let nbt = serde_json::to_string_pretty(&block_entity.nbt)
                .context("序列化方块实体 NBT 失败")?;
            for line in nbt.lines() {
                println!("        {line}");
            }
        }
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct InspectReport<'a> {
    format: &'a str,
    data_version: Option<i32>,
    bounds: InspectBounds,
    non_air_blocks: usize,
    sections: usize,
    block_types: &'a BTreeMap<String, usize>,
    unsupported_active_blocks: Vec<String>,
    blocks: Vec<BlockInspection>,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct InspectBounds {
    min: BlockPos,
    max: BlockPos,
}

#[derive(Debug, Serialize)]
struct BlockInspection {
    position: BlockPos,
    state: BlockStateInspection,
    #[serde(skip_serializing_if = "Option::is_none")]
    block_entity: Option<BlockEntityInspection>,
}

#[derive(Debug, Serialize)]
struct BlockStateInspection {
    id: BlockStateId,
    name: String,
    properties: BTreeMap<String, String>,
    supported: bool,
}

#[derive(Debug, Serialize)]
struct BlockEntityInspection {
    id: String,
    nbt: BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_position_requires_three_signed_integers() {
        assert_eq!(parse_block_pos("-1, 2, 30"), Ok(BlockPos::new(-1, 2, 30)));
        assert!(parse_block_pos("1,2").is_err());
        assert!(parse_block_pos("1,two,3").is_err());
    }
}
