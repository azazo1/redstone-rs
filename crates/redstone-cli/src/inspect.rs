use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{
    LoadedStructure, StructureLoadOptions, StructureLoader, StructureRegion, StructureTransform,
};
use redstone_java_26::{Java26Registry, StateDefinition};
use serde::Serialize;

use crate::{RegistryResolver, reject_newer_data_version};

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

pub(crate) fn parse_block_pos(value: &str) -> Result<BlockPos, String> {
    let coordinates = value.split(',').map(str::trim).collect::<Vec<_>>();
    if coordinates.len() != 3 {
        return Err("方块坐标必须使用 X,Y,Z 格式".to_owned());
    }
    Ok(BlockPos::new(
        parse_coordinate(coordinates[0])?,
        parse_coordinate(coordinates[1])?,
        parse_coordinate(coordinates[2])?,
    ))
}

pub(crate) fn region_from_corners(corners: &[BlockPos]) -> Option<StructureRegion> {
    match corners {
        [] => None,
        [first, second] => Some(StructureRegion::new(*first, *second)),
        _ => unreachable!("clap 保证 region 包含两个坐标"),
    }
}

fn parse_coordinate(value: &str) -> Result<i32, String> {
    let value = value.trim();
    value
        .parse::<i32>()
        .map_err(|_| format!("坐标值不是有效的 32 位整数: {value}"))
}

pub fn run(
    path: &Path,
    region: Option<StructureRegion>,
    requested_blocks: &[BlockPos],
    all: bool,
    requested_types: &[String],
    output_format: OutputFormat,
) -> Result<()> {
    let mut resolver = RegistryResolver(Java26Registry::new());
    let load_region = (StructureLoader::detect(path)? == redstone_io::StructureFormat::MinecraftWorld)
        .then_some(region)
        .flatten();
    let structure = StructureLoader::load_with_options(
        path,
        StructureLoadOptions {
            transform: StructureTransform::default(),
            region: load_region,
        },
        &mut resolver,
    )?;
    reject_newer_data_version(structure.data_version)?;

    let unsupported_active_blocks = resolver
        .0
        .states()
        .filter(|state| !state.supported)
        .map(|state| state.name.to_string())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let positions = selected_positions(
        &structure,
        &resolver.0,
        requested_blocks,
        region,
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
        region_bounds: InspectBounds {
            min: structure.region_min,
            max: structure.region_max,
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
    requested_blocks: &[BlockPos],
    requested_region: Option<StructureRegion>,
    all: bool,
    requested_types: &[String],
) -> Result<Vec<BlockPos>> {
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
    let matches_type = |state_id| {
        requested_types.is_empty()
            || registry
                .state(state_id)
                .is_some_and(|state| requested_types.contains(state.name.as_ref()))
    };

    if !requested_blocks.is_empty() || requested_region.is_some() {
        let mut positions = BTreeSet::new();
        for pos in requested_blocks {
            if matches_type(structure.world.get_block(*pos)) {
                positions.insert(*pos);
            }
        }
        if let Some(region) = requested_region {
            positions.extend(structure.world.iter_blocks().filter_map(|(pos, state_id)| {
                (matches_type(state_id) && region.contains(pos)).then_some(pos)
            }));
        }
        return Ok(positions.into_iter().collect());
    }

    if !all && requested_types.is_empty() {
        return Ok(Vec::new());
    }
    Ok(structure
        .world
        .iter_blocks()
        .filter_map(|(pos, state_id)| (all || matches_type(state_id)).then_some(pos))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
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
                    nbt.insert(
                        "id".to_owned(),
                        serde_json::Value::String(data.kind.clone()),
                    );
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
        name: state.name.to_string(),
        properties: state.properties.as_ref().clone(),
        supported: state.supported,
    }
}

fn write_text(report: &InspectReport<'_>) -> Result<()> {
    println!("format: {}", report.format);
    println!("data_version: {:?}", report.data_version);
    println!("bounds: {:?} .. {:?}", report.bounds.min, report.bounds.max);
    println!(
        "region_bounds: {:?} .. {:?}",
        report.region_bounds.min, report.region_bounds.max
    );
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
    region_bounds: InspectBounds,
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
    fn block_position_accepts_only_single_coordinates() {
        assert_eq!(
            parse_block_pos("-1, 2, 30").unwrap(),
            BlockPos::new(-1, 2, 30)
        );
        assert!(parse_block_pos("1,2").is_err());
        assert!(parse_block_pos("1,two,3").is_err());
        assert!(parse_block_pos("0..1,2,3").is_err());
    }

    #[test]
    fn region_uses_two_inclusive_block_coordinates() {
        let region = region_from_corners(&[
            BlockPos::new(3, 5, 0),
            BlockPos::new(1, 2, -4),
        ])
        .unwrap();
        assert_eq!(region.min, BlockPos::new(1, 2, -4));
        assert_eq!(region.max, BlockPos::new(3, 5, 0));
        assert!(region_from_corners(&[]).is_none());
    }
}
