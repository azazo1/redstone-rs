use std::collections::{BTreeMap, HashMap};

use fastnbt::Value;
use redstone_core::{BlockPos, BlockStateId};
use thiserror::Error;
use tracing::info;

use super::LoadedStructure;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct VanillaState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

pub fn encode_vanilla_structure(
    structure: &LoadedStructure,
    include_air: bool,
    data_version: i32,
    mut describe_state: impl FnMut(BlockStateId) -> Result<VanillaState, String>,
) -> Result<Vec<u8>, VanillaWriteError> {
    let min = structure.region_min;
    let max = structure.region_max;
    let size = [
        dimension(min.x, max.x)?,
        dimension(min.y, max.y)?,
        dimension(min.z, max.z)?,
    ];
    let mut palette = BTreeMap::<VanillaState, usize>::new();
    let mut blocks = Vec::new();
    let total = size
        .iter()
        .try_fold(1usize, |volume, dimension| volume.checked_mul(*dimension as usize))
        .ok_or(VanillaWriteError::VolumeOverflow)?;
    let mut processed = 0usize;
    if include_air {
        for y in min.y..=max.y {
            for z in min.z..=max.z {
                for x in min.x..=max.x {
                    push_block(
                        structure,
                        BlockPos::new(x, y, z),
                        min,
                        &mut palette,
                        &mut blocks,
                        &mut describe_state,
                    )?;
                    processed += 1;
                    if processed.is_multiple_of(1_000_000) {
                        info!(processed, total, "转换 vanilla structure 方块");
                    }
                }
            }
        }
    } else {
        for (pos, _) in structure.world.iter_blocks() {
            if !in_region(pos, min, max) {
                continue;
            }
            push_block(
                structure,
                pos,
                min,
                &mut palette,
                &mut blocks,
                &mut describe_state,
            )?;
            processed += 1;
            if processed.is_multiple_of(250_000) {
                info!(processed, "转换 vanilla structure 非空气方块");
            }
        }
    }
    let mut ordered_palette = vec![Value::Compound(HashMap::new()); palette.len()];
    for (state, index) in palette {
        let mut encoded = HashMap::from([("Name".to_owned(), Value::String(state.name))]);
        if !state.properties.is_empty() {
            encoded.insert(
                "Properties".to_owned(),
                Value::Compound(
                    state
                        .properties
                        .into_iter()
                        .map(|(name, value)| (name, Value::String(value)))
                        .collect(),
                ),
            );
        }
        ordered_palette[index] = Value::Compound(encoded);
    }
    let entities = structure
        .world
        .entities()
        .filter_map(|(_, entity)| {
            let pos = [
                entity.position[0] - min.x as f64,
                entity.position[1] - min.y as f64,
                entity.position[2] - min.z as f64,
            ];
            let block_pos = [
                floor_to_i32(pos[0])?,
                floor_to_i32(pos[1])?,
                floor_to_i32(pos[2])?,
            ];
            let mut nbt = entity
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), json_to_nbt(value)))
                .collect::<HashMap<_, _>>();
            nbt.insert("id".to_owned(), Value::String(entity.kind.clone()));
            Some(Value::Compound(HashMap::from([
                (
                    "pos".to_owned(),
                    Value::List(pos.into_iter().map(Value::Double).collect()),
                ),
                (
                    "blockPos".to_owned(),
                    Value::List(block_pos.into_iter().map(Value::Int).collect()),
                ),
                ("nbt".to_owned(), Value::Compound(nbt)),
            ])))
        })
        .collect::<Vec<_>>();
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(data_version)),
        (
            "size".to_owned(),
            Value::List(size.into_iter().map(Value::Int).collect()),
        ),
        ("palette".to_owned(), Value::List(ordered_palette)),
        ("blocks".to_owned(), Value::List(blocks)),
        ("entities".to_owned(), Value::List(entities)),
    ]);
    info!(
        blocks = processed,
        states = ordered_palette_len(&root),
        include_air,
        "完成 vanilla structure 转换"
    );
    fastnbt::to_bytes(&root).map_err(VanillaWriteError::Nbt)
}

fn push_block(
    structure: &LoadedStructure,
    pos: BlockPos,
    origin: BlockPos,
    palette: &mut BTreeMap<VanillaState, usize>,
    blocks: &mut Vec<Value>,
    describe_state: &mut impl FnMut(BlockStateId) -> Result<VanillaState, String>,
) -> Result<(), VanillaWriteError> {
    let state = structure.world.get_block(pos);
    let description = describe_state(state).map_err(VanillaWriteError::State)?;
    let next_index = palette.len();
    let state_index = *palette.entry(description).or_insert(next_index);
    let mut block = HashMap::from([
        (
            "pos".to_owned(),
            Value::List(vec![
                Value::Int(pos.x - origin.x),
                Value::Int(pos.y - origin.y),
                Value::Int(pos.z - origin.z),
            ]),
        ),
        ("state".to_owned(), Value::Int(state_index as i32)),
    ]);
    if let Some(data) = structure.world.block_entity(pos) {
        let mut nbt = structure
            .block_entity_nbt
            .get(&pos)
            .map(|fields| {
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), json_to_nbt(value)))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_else(|| {
                data.fields
                    .iter()
                    .map(|(name, value)| (name.clone(), json_to_nbt(value)))
                    .collect()
            });
        nbt.insert("id".to_owned(), Value::String(data.kind.clone()));
        nbt.remove("x");
        nbt.remove("y");
        nbt.remove("z");
        block.insert("nbt".to_owned(), Value::Compound(nbt));
    }
    blocks.push(Value::Compound(block));
    Ok(())
}

fn json_to_nbt(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::String(String::new()),
        serde_json::Value::Bool(value) => Value::Byte(i8::from(*value)),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map_or_else(
                || Value::Double(value.as_f64().unwrap_or_default()),
                |value| i32::try_from(value).map_or(Value::Long(value), Value::Int),
            ),
        serde_json::Value::String(value) => Value::String(value.clone()),
        serde_json::Value::Array(values) => {
            Value::List(values.iter().map(json_to_nbt).collect())
        }
        serde_json::Value::Object(values) => Value::Compound(
            values
                .iter()
                .map(|(name, value)| (name.clone(), json_to_nbt(value)))
                .collect(),
        ),
    }
}

fn dimension(min: i32, max: i32) -> Result<i32, VanillaWriteError> {
    max.checked_sub(min)
        .and_then(|value| value.checked_add(1))
        .filter(|value| *value > 0)
        .ok_or(VanillaWriteError::InvalidBounds)
}

fn in_region(pos: BlockPos, min: BlockPos, max: BlockPos) -> bool {
    (min.x..=max.x).contains(&pos.x)
        && (min.y..=max.y).contains(&pos.y)
        && (min.z..=max.z).contains(&pos.z)
}

fn floor_to_i32(value: f64) -> Option<i32> {
    let value = value.floor();
    (value >= i32::MIN as f64 && value <= i32::MAX as f64).then_some(value as i32)
}

fn ordered_palette_len(root: &HashMap<String, Value>) -> usize {
    root.get("palette")
        .and_then(|value| match value {
            Value::List(values) => Some(values.len()),
            _ => None,
        })
        .unwrap_or(0)
}

#[derive(Debug, Error)]
pub enum VanillaWriteError {
    #[error("结构区域边界无效")]
    InvalidBounds,
    #[error("结构区域体积溢出")]
    VolumeOverflow,
    #[error("无法导出方块状态: {0}")]
    State(String),
    #[error("NBT 编码失败: {0}")]
    Nbt(fastnbt::error::Error),
}
