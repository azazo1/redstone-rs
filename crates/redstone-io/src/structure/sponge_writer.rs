use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use fastnbt::{ByteArray, IntArray, Value};
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};

use super::{
    LoadedStructure,
    writer::{StructureState, StructureWriteError},
};

pub(super) fn encode(
    structure: &LoadedStructure,
    data_version: i32,
    mut describe_state: impl FnMut(BlockStateId) -> Result<StructureState, String>,
) -> Result<Vec<u8>, StructureWriteError> {
    let min = structure.region_min;
    let dimensions = dimensions(structure)?;
    let volume = dimensions
        .iter()
        .try_fold(1usize, |volume, value| volume.checked_mul(*value as usize))
        .ok_or_else(|| StructureWriteError::Sponge("区域体积溢出".to_owned()))?;
    let mut palette = BTreeMap::<StructureState, usize>::new();
    let mut data = Vec::<i8>::new();
    data.try_reserve_exact(volume)
        .map_err(|error| StructureWriteError::Sponge(error.to_string()))?;
    for y in 0..dimensions[1] {
        for z in 0..dimensions[2] {
            for x in 0..dimensions[0] {
                let state = structure.world.get_block(min.offset(x, y, z));
                let description = describe_state(state).map_err(StructureWriteError::Sponge)?;
                let next = palette.len();
                encode_varint(*palette.entry(description).or_insert(next) as u32, &mut data);
            }
        }
    }
    let encoded_palette = palette
        .into_iter()
        .map(|(state, index)| (state_string(&state), Value::Int(index as i32)))
        .collect::<HashMap<_, _>>();
    let block_entities = structure
        .world
        .block_entities()
        .filter(|(pos, _)| in_region(**pos, structure.region_min, structure.region_max))
        .map(|(pos, block_entity)| {
            let mut raw = structure
                .block_entity_nbt
                .get(pos)
                .map(|fields| {
                    fields
                        .iter()
                        .filter(|(key, _)| !matches!(key.as_str(), "id" | "x" | "y" | "z"))
                        .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                        .collect()
                })
                .unwrap_or_else(|| {
                    block_entity
                        .fields
                        .iter()
                        .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                        .collect()
                });
            Value::Compound(HashMap::from([
                ("Id".to_owned(), Value::String(block_entity.kind.clone())),
                (
                    "Pos".to_owned(),
                    Value::IntArray(IntArray::new(vec![
                        pos.x - min.x,
                        pos.y - min.y,
                        pos.z - min.z,
                    ])),
                ),
                ("Data".to_owned(), Value::Compound(std::mem::take(&mut raw))),
            ]))
        })
        .collect::<Vec<_>>();
    let entities = structure
        .world
        .entities()
        .map(|(_, entity)| {
            let data = entity
                .fields
                .iter()
                .map(|(key, value)| (key.clone(), json_to_nbt(value)))
                .collect();
            Value::Compound(HashMap::from([
                ("Id".to_owned(), Value::String(entity.kind.clone())),
                (
                    "Pos".to_owned(),
                    Value::List(vec![
                        Value::Double(entity.position[0] - min.x as f64),
                        Value::Double(entity.position[1] - min.y as f64),
                        Value::Double(entity.position[2] - min.z as f64),
                    ]),
                ),
                ("Data".to_owned(), Value::Compound(data)),
            ]))
        })
        .collect::<Vec<_>>();
    let blocks = HashMap::from([
        ("Palette".to_owned(), Value::Compound(encoded_palette)),
        ("Data".to_owned(), Value::ByteArray(ByteArray::new(data))),
        (
            "BlockEntities".to_owned(),
            Value::List(block_entities),
        ),
    ]);
    let schematic = HashMap::from([
        ("Version".to_owned(), Value::Int(3)),
        ("DataVersion".to_owned(), Value::Int(data_version)),
        ("Width".to_owned(), Value::Short(dimensions[0] as i16)),
        ("Height".to_owned(), Value::Short(dimensions[1] as i16)),
        ("Length".to_owned(), Value::Short(dimensions[2] as i16)),
        (
            "Offset".to_owned(),
            Value::IntArray(IntArray::new(vec![min.x, min.y, min.z])),
        ),
        ("Blocks".to_owned(), Value::Compound(blocks)),
        ("Entities".to_owned(), Value::List(entities)),
    ]);
    let root = HashMap::from([("Schematic".to_owned(), Value::Compound(schematic))]);
    let nbt = fastnbt::to_bytes(&root)
        .map_err(|error| StructureWriteError::Sponge(error.to_string()))?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&nbt)?;
    encoder.finish().map_err(StructureWriteError::Io)
}

fn dimensions(structure: &LoadedStructure) -> Result<[i32; 3], StructureWriteError> {
    let dimension = |min: i32, max: i32| {
        max.checked_sub(min)
            .and_then(|value| value.checked_add(1))
            .filter(|value| (1..=u16::MAX as i32).contains(value))
            .ok_or_else(|| StructureWriteError::Sponge("区域轴长超出 u16 范围".to_owned()))
    };
    Ok([
        dimension(structure.region_min.x, structure.region_max.x)?,
        dimension(structure.region_min.y, structure.region_max.y)?,
        dimension(structure.region_min.z, structure.region_max.z)?,
    ])
}

fn state_string(state: &StructureState) -> String {
    if state.properties.is_empty() {
        return state.name.clone();
    }
    format!(
        "{}[{}]",
        state.name,
        state
            .properties
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(",")
    )
}

fn encode_varint(mut value: u32, output: &mut Vec<i8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte as i8);
        if value == 0 {
            return;
        }
    }
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
