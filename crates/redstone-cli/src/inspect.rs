use std::collections::{BTreeMap, BTreeSet};
use std::ops::Bound;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BlockSelector {
    region: BlockRegion,
    point: Option<BlockPos>,
}

impl BlockSelector {
    fn matches(self, pos: BlockPos) -> bool {
        self.region.x.contains(pos.x)
            && self.region.y.contains(pos.y)
            && self.region.z.contains(pos.z)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BlockRegion {
    x: AxisRange,
    y: AxisRange,
    z: AxisRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AxisRange {
    start: Bound<i32>,
    end: Bound<i32>,
}

impl AxisRange {
    fn point(value: i32) -> Self {
        Self {
            start: Bound::Included(value),
            end: Bound::Included(value),
        }
    }

    fn new(start: Option<i32>, end: Option<i32>, inclusive_end: bool) -> Self {
        let (start, end) = match (start, end) {
            (Some(start), Some(end)) if start > end => (Some(end), Some(start)),
            bounds => bounds,
        };
        Self {
            start: start.map_or(Bound::Unbounded, Bound::Included),
            end: end.map_or(Bound::Unbounded, |value| {
                if inclusive_end {
                    Bound::Included(value)
                } else {
                    Bound::Excluded(value)
                }
            }),
        }
    }

    fn contains(self, value: i32) -> bool {
        let after_start = match self.start {
            Bound::Included(start) => value >= start,
            Bound::Excluded(start) => value > start,
            Bound::Unbounded => true,
        };
        let before_end = match self.end {
            Bound::Included(end) => value <= end,
            Bound::Excluded(end) => value < end,
            Bound::Unbounded => true,
        };
        after_start && before_end
    }
}

pub(crate) fn parse_block_selector(value: &str) -> Result<BlockSelector, String> {
    let coordinates = value.split(',').map(str::trim).collect::<Vec<_>>();
    if coordinates.len() != 3 {
        return Err("方块选择器必须使用 X,Y,Z 格式".to_owned());
    }
    let (x, point_x) = parse_axis_range(coordinates[0])?;
    let (y, point_y) = parse_axis_range(coordinates[1])?;
    let (z, point_z) = parse_axis_range(coordinates[2])?;
    Ok(BlockSelector {
        region: BlockRegion { x, y, z },
        point: match (point_x, point_y, point_z) {
            (Some(x), Some(y), Some(z)) => Some(BlockPos::new(x, y, z)),
            _ => None,
        },
    })
}

pub(crate) fn parse_finite_region(value: &str) -> Result<StructureRegion, String> {
    let coordinates = value.split(',').map(str::trim).collect::<Vec<_>>();
    if coordinates.len() != 3 {
        return Err("region 必须使用 X_RANGE,Y_RANGE,Z_RANGE 格式".to_owned());
    }
    let x = finite_axis(coordinates[0])?;
    let y = finite_axis(coordinates[1])?;
    let z = finite_axis(coordinates[2])?;
    Ok(StructureRegion::new(
        BlockPos::new(x.0, y.0, z.0),
        BlockPos::new(x.1, y.1, z.1),
    ))
}

fn finite_axis(value: &str) -> Result<(i32, i32), String> {
    let (axis, point) = parse_axis_range(value)?;
    if let Some(point) = point {
        return Ok((point, point));
    }
    let start = match axis.start {
        Bound::Included(value) => value,
        _ => return Err(format!("region 范围缺少有限下界: {value}")),
    };
    let end = match axis.end {
        Bound::Included(value) => value,
        Bound::Excluded(value) => value
            .checked_sub(1)
            .ok_or_else(|| format!("region 半开范围上界溢出: {value}"))?,
        Bound::Unbounded => return Err(format!("region 范围缺少有限上界: {value}")),
    };
    if start > end {
        return Err(format!("region 范围为空: {value}"));
    }
    Ok((start, end))
}

fn parse_axis_range(value: &str) -> Result<(AxisRange, Option<i32>), String> {
    if let Some((start, end)) = value.split_once("..=") {
        reject_additional_range_operator(start, end, value)?;
        if end.trim().is_empty() {
            return Err(format!("闭区间范围缺少上界: {value}"));
        }
        return Ok((
            AxisRange::new(parse_optional_coordinate(start)?, Some(parse_coordinate(end)?), true),
            None,
        ));
    }
    if let Some((start, end)) = value.split_once("..") {
        reject_additional_range_operator(start, end, value)?;
        return Ok((
            AxisRange::new(
                parse_optional_coordinate(start)?,
                parse_optional_coordinate(end)?,
                false,
            ),
            None,
        ));
    }

    let coordinate = parse_coordinate(value)?;
    Ok((AxisRange::point(coordinate), Some(coordinate)))
}

fn reject_additional_range_operator(start: &str, end: &str, value: &str) -> Result<(), String> {
    if start.contains("..") || end.contains("..") {
        return Err(format!("范围包含多个运算符: {value}"));
    }
    Ok(())
}

fn parse_optional_coordinate(value: &str) -> Result<Option<i32>, String> {
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        parse_coordinate(value).map(Some)
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
    requested_blocks: &[BlockSelector],
    all: bool,
    requested_types: &[String],
    output_format: OutputFormat,
) -> Result<()> {
    let mut resolver = RegistryResolver(Java26Registry::new());
    let structure = StructureLoader::load_with_options(
        path,
        StructureLoadOptions {
            transform: StructureTransform::default(),
            region,
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
    requested_blocks: &[BlockSelector],
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

    if !requested_blocks.is_empty() {
        let mut positions = BTreeSet::new();
        for selector in requested_blocks {
            let Some(pos) = selector.point else {
                continue;
            };
            if matches_type(structure.world.get_block(pos)) {
                positions.insert(pos);
            }
        }

        let regions = requested_blocks
            .iter()
            .filter(|selector| selector.point.is_none())
            .copied()
            .collect::<Vec<_>>();
        if !regions.is_empty() {
            positions.extend(structure.world.iter_blocks().filter_map(|(pos, state_id)| {
                (matches_type(state_id) && regions.iter().any(|region| region.matches(pos)))
                    .then_some(pos)
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
    fn block_selector_parses_points_and_mixed_ranges() {
        let point = parse_block_selector("-1, 2, 30").unwrap();
        assert_eq!(point.point, Some(BlockPos::new(-1, 2, 30)));

        let range = parse_block_selector("-3..=-1, 2..5, ..").unwrap();
        assert_eq!(range.point, None);
        assert!(range.matches(BlockPos::new(-3, 2, i32::MIN)));
        assert!(range.matches(BlockPos::new(-1, 4, i32::MAX)));
        assert!(!range.matches(BlockPos::new(0, 4, 0)));
        assert!(!range.matches(BlockPos::new(-2, 5, 0)));
    }

    #[test]
    fn block_selector_supports_open_and_reversed_ranges() {
        let open = parse_block_selector("..=-1, 3.., ..10").unwrap();
        assert!(open.matches(BlockPos::new(-1, 3, 9)));
        assert!(!open.matches(BlockPos::new(0, 3, 9)));
        assert!(!open.matches(BlockPos::new(-1, 2, 9)));
        assert!(!open.matches(BlockPos::new(-1, 3, 10)));

        let reversed = parse_block_selector("3..1, 5..=2, 0").unwrap();
        assert!(reversed.matches(BlockPos::new(1, 2, 0)));
        assert!(reversed.matches(BlockPos::new(2, 5, 0)));
        assert!(!reversed.matches(BlockPos::new(3, 5, 0)));
    }

    #[test]
    fn block_selector_rejects_invalid_syntax() {
        assert!(parse_block_selector("1,2").is_err());
        assert!(parse_block_selector("1,two,3").is_err());
        assert!(parse_block_selector("1..=,2,3").is_err());
        assert!(parse_block_selector("1..2..3,0,0").is_err());
    }

    #[test]
    fn finite_region_requires_bounded_non_empty_axes() {
        let region = parse_finite_region("-2..=3,0..16,5").unwrap();
        assert_eq!(region.min, BlockPos::new(-2, 0, 5));
        assert_eq!(region.max, BlockPos::new(3, 15, 5));
        assert!(parse_finite_region("..3,0..1,0..1").is_err());
        assert!(parse_finite_region("0..,0..1,0..1").is_err());
        assert!(parse_finite_region("0..0,0..1,0..1").is_err());
    }
}
