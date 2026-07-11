use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use fastnbt::{LongArray, Value};
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};

use super::{
    LoadedStructure,
    writer::{StructureState, StructureWriteError, WriteProgress},
};

pub(super) fn encode(
    structure: &LoadedStructure,
    data_version: i32,
    progress: &mut WriteProgress,
    mut describe_state: impl FnMut(BlockStateId) -> Result<StructureState, String>,
) -> Result<Vec<u8>, StructureWriteError> {
    let min = structure.region_min;
    let dimensions = dimensions(structure)?;
    let volume = dimensions
        .iter()
        .try_fold(1usize, |volume, value| volume.checked_mul(*value as usize))
        .ok_or_else(|| StructureWriteError::Litematic("区域体积溢出".to_owned()))?;
    let mut palette = BTreeMap::<StructureState, usize>::new();
    let mut indices = Vec::new();
    indices
        .try_reserve_exact(volume)
        .map_err(|error| StructureWriteError::Litematic(error.to_string()))?;
    for y in 0..dimensions[1] {
        for z in 0..dimensions[2] {
            for x in 0..dimensions[0] {
                let state = structure.world.get_block(min.offset(x, y, z));
                let description = describe_state(state).map_err(StructureWriteError::Litematic)?;
                let next = palette.len();
                indices.push(*palette.entry(description).or_insert(next));
                progress.advance_encoding();
            }
        }
    }
    let bits = bits_for_palette(palette.len());
    let long_count = volume
        .checked_mul(bits)
        .and_then(|bits| bits.checked_add(63))
        .map(|bits| bits / 64)
        .ok_or_else(|| StructureWriteError::Litematic("方块状态数组长度溢出".to_owned()))?;
    let mut packed = vec![0u64; long_count];
    for (index, value) in indices.into_iter().enumerate() {
        let bit_index = index * bits;
        let start = bit_index / 64;
        let offset = bit_index % 64;
        packed[start] |= (value as u64) << offset;
        if offset + bits > 64 {
            packed[start + 1] |= (value as u64) >> (64 - offset);
        }
    }
    let mut encoded_palette = vec![Value::Compound(HashMap::new()); palette.len()];
    for (state, index) in palette {
        let mut entry = HashMap::from([("Name".to_owned(), Value::String(state.name))]);
        if !state.properties.is_empty() {
            entry.insert(
                "Properties".to_owned(),
                Value::Compound(
                    state
                        .properties
                        .into_iter()
                        .map(|(key, value)| (key, Value::String(value)))
                        .collect(),
                ),
            );
        }
        encoded_palette[index] = Value::Compound(entry);
    }
    let tile_entities = structure
        .world
        .block_entities()
        .filter(|(pos, _)| in_region(**pos, structure.region_min, structure.region_max))
        .map(|(pos, data)| {
            let mut nbt = raw_block_entity(structure, *pos, data);
            nbt.insert("x".to_owned(), Value::Int(pos.x - min.x));
            nbt.insert("y".to_owned(), Value::Int(pos.y - min.y));
            nbt.insert("z".to_owned(), Value::Int(pos.z - min.z));
            Value::Compound(nbt)
        })
        .collect::<Vec<_>>();
    let entities = structure
        .world
        .entities()
        .map(|(_, entity)| {
            let mut nbt = entity
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                .collect::<HashMap<_, _>>();
            nbt.insert("id".to_owned(), Value::String(entity.kind.clone()));
            nbt.insert(
                "Pos".to_owned(),
                Value::List(vec![
                    Value::Double(entity.position[0] - min.x as f64),
                    Value::Double(entity.position[1] - min.y as f64),
                    Value::Double(entity.position[2] - min.z as f64),
                ]),
            );
            Value::Compound(nbt)
        })
        .collect::<Vec<_>>();
    let region = HashMap::from([
        ("Position".to_owned(), xyz(min.x, min.y, min.z)),
        (
            "Size".to_owned(),
            xyz(dimensions[0], dimensions[1], dimensions[2]),
        ),
        ("BlockStatePalette".to_owned(), Value::List(encoded_palette)),
        (
            "BlockStates".to_owned(),
            Value::LongArray(LongArray::new(
                packed.into_iter().map(|value| value as i64).collect(),
            )),
        ),
        ("TileEntities".to_owned(), Value::List(tile_entities)),
        ("Entities".to_owned(), Value::List(entities)),
    ]);
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(7)),
        ("SubVersion".to_owned(), Value::Int(1)),
        ("MinecraftDataVersion".to_owned(), Value::Int(data_version)),
        (
            "Metadata".to_owned(),
            Value::Compound(HashMap::from([
                ("Name".to_owned(), Value::String("redstone-rs".to_owned())),
                ("Author".to_owned(), Value::String("redstone-rs".to_owned())),
                ("Description".to_owned(), Value::String(String::new())),
                ("RegionCount".to_owned(), Value::Int(1)),
                ("TimeCreated".to_owned(), Value::Long(0)),
                ("TimeModified".to_owned(), Value::Long(0)),
                ("TotalBlocks".to_owned(), Value::Int(structure.world.non_air_blocks() as i32)),
                ("TotalVolume".to_owned(), Value::Long(volume as i64)),
                (
                    "EnclosingSize".to_owned(),
                    xyz(dimensions[0], dimensions[1], dimensions[2]),
                ),
            ])),
        ),
        (
            "Regions".to_owned(),
            Value::Compound(HashMap::from([(
                "main".to_owned(),
                Value::Compound(region),
            )])),
        ),
    ]);
    let nbt = fastnbt::to_bytes(&root)
        .map_err(|error| StructureWriteError::Litematic(error.to_string()))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt)?;
    encoder.finish().map_err(StructureWriteError::Io)
}

fn dimensions(structure: &LoadedStructure) -> Result<[i32; 3], StructureWriteError> {
    let dimension = |min: i32, max: i32| {
        max.checked_sub(min)
            .and_then(|value| value.checked_add(1))
            .filter(|value| *value > 0)
            .ok_or_else(|| StructureWriteError::Litematic("区域边界无效".to_owned()))
    };
    Ok([
        dimension(structure.region_min.x, structure.region_max.x)?,
        dimension(structure.region_min.y, structure.region_max.y)?,
        dimension(structure.region_min.z, structure.region_max.z)?,
    ])
}

fn bits_for_palette(size: usize) -> usize {
    (usize::BITS - size.saturating_sub(1).leading_zeros()).max(2) as usize
}

fn xyz(x: i32, y: i32, z: i32) -> Value {
    Value::Compound(HashMap::from([
        ("x".to_owned(), Value::Int(x)),
        ("y".to_owned(), Value::Int(y)),
        ("z".to_owned(), Value::Int(z)),
    ]))
}

fn raw_block_entity(
    structure: &LoadedStructure,
    pos: BlockPos,
    data: &redstone_core::BlockEntityData,
) -> HashMap<String, Value> {
    let mut nbt: HashMap<String, Value> = structure
        .block_entity_nbt
        .get(&pos)
        .map(|fields| {
            fields
                .iter()
                .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                .collect()
        })
        .unwrap_or_else(|| {
            data.fields
                .iter()
                .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                .collect()
        });
    nbt.insert("id".to_owned(), Value::String(data.kind.clone()));
    nbt
}

fn json_to_nbt(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::String(String::new()),
        serde_json::Value::Bool(value) => Value::Byte(i8::from(*value)),
        serde_json::Value::Number(value) => value.as_i64().map_or_else(
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
                .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                .collect(),
        ),
    }
}

fn in_region(pos: BlockPos, min: BlockPos, max: BlockPos) -> bool {
    (min.x..=max.x).contains(&pos.x)
        && (min.y..=max.y).contains(&pos.y)
        && (min.z..=max.z).contains(&pos.z)
}
