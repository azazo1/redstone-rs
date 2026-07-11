use std::collections::{BTreeMap, HashMap};

use fastnbt::Value;
use redstone_core::BlockPos;

use super::LoadState;
use super::super::{
    StructureLoadOptions, StructureRegion, StructureStateResolver, entity_from_nbt,
    nbt_compound_to_json, nbt_to_block_entity, resolve, transform_entity, value_i32,
};

#[derive(Clone, Copy)]
pub(super) struct ChunkLoadOptions {
    pub region: Option<StructureRegion>,
    pub load: StructureLoadOptions,
    pub separate_entities: bool,
}

pub(super) fn load_chunk<R: StructureStateResolver>(
    root: &HashMap<String, Value>,
    chunk: (i32, i32),
    options: ChunkLoadOptions,
    resolver: &mut R,
    state: &mut LoadState,
) -> Result<(), String> {
    let (chunk_x, chunk_z) = chunk;
    let sections = optional_list(root, "sections")?;
    for section_value in sections.into_iter().flatten() {
        let section = compound_value(section_value)?;
        let section_y = section
            .get("Y")
            .and_then(value_i32)
            .ok_or_else(|| "chunk section 缺少 Y".to_owned())?;
        let Some(Value::Compound(block_states)) = section.get("block_states") else {
            continue;
        };
        let palette_values = list(block_states, "palette")?;
        if palette_values.is_empty() {
            return Err("chunk block_states palette 为空".to_owned());
        }
        let mut palette = Vec::with_capacity(palette_values.len());
        let mut names = Vec::with_capacity(palette_values.len());
        for entry in palette_values {
            let entry = compound_value(entry)?;
            let name = string(entry, "Name")?;
            let properties = properties(entry)?;
            palette.push(resolve(resolver, name, &properties).map_err(|error| error.to_string())?);
            names.push(name.to_owned());
        }
        let data = match block_states.get("data") {
            Some(Value::LongArray(values)) => Some(values.as_ref()),
            Some(_) => return Err("chunk block_states.data 类型无效".to_owned()),
            None => None,
        };
        if palette.len() > 1 && data.is_none() {
            return Err("多状态 chunk section 缺少 data".to_owned());
        }
        let bits = bits_for_palette(palette.len());
        let values_per_long = 64 / bits;
        for index in 0..4096usize {
            let palette_index = if palette.len() == 1 {
                0
            } else {
                let longs = data.unwrap();
                let long = *longs
                    .get(index / values_per_long)
                    .ok_or_else(|| "chunk block_states.data 长度不足".to_owned())?
                    as u64;
                ((long >> ((index % values_per_long) * bits)) & ((1u64 << bits) - 1)) as usize
            };
            let block_state = *palette
                .get(palette_index)
                .ok_or_else(|| format!("chunk palette 索引越界: {palette_index}"))?;
            if block_state == resolver.air_state() {
                continue;
            }
            let local_x = (index % 16) as i32;
            let local_z = ((index / 16) % 16) as i32;
            let local_y = (index / 256) as i32;
            let source = BlockPos::new(
                chunk_x
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(local_x))
                    .ok_or_else(|| "方块 x 坐标溢出".to_owned())?,
                section_y
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(local_y))
                    .ok_or_else(|| "方块 y 坐标溢出".to_owned())?,
                chunk_z
                    .checked_mul(16)
                    .and_then(|value| value.checked_add(local_z))
                    .ok_or_else(|| "方块 z 坐标溢出".to_owned())?,
            );
            if options.region.is_some_and(|region| !region.contains(source)) {
                continue;
            }
            let target = options.load.transform.apply(source);
            state
                .world
                .set_block(target, block_state)
                .map_err(|error| error.to_string())?;
            state.include_block(target);
            *state
                .block_counts
                .entry(names[palette_index].clone())
                .or_default() += 1;
        }
    }
    if let Some(block_entities) = optional_list(root, "block_entities")? {
        for value in block_entities {
            let nbt = compound_value(value)?;
            let source = BlockPos::new(
                integer(nbt, "x")?,
                integer(nbt, "y")?,
                integer(nbt, "z")?,
            );
            if options.region.is_some_and(|region| !region.contains(source)) {
                continue;
            }
            let target = options.load.transform.apply(source);
            let mut normalized = nbt.clone();
            normalized.insert("x".to_owned(), Value::Int(target.x));
            normalized.insert("y".to_owned(), Value::Int(target.y));
            normalized.insert("z".to_owned(), Value::Int(target.z));
            state
                .block_entity_nbt
                .insert(target, nbt_compound_to_json(&normalized));
            state
                .world
                .set_block_entity(target, nbt_to_block_entity(&normalized));
            state.include_content(target);
        }
    }
    if !options.separate_entities {
        load_entities(root, "entities", options.region, options.load, state)?;
    }
    Ok(())
}

pub(super) fn load_entities(
    root: &HashMap<String, Value>,
    key: &str,
    region: Option<StructureRegion>,
    options: StructureLoadOptions,
    state: &mut LoadState,
) -> Result<(), String> {
    let Some(entities) = optional_list(root, key)? else {
        return Ok(());
    };
    for value in entities {
        let nbt = compound_value(value)?;
        let Some(entity) = entity_from_nbt(nbt).map_err(|error| error.to_string())? else {
            continue;
        };
        let source = BlockPos::new(
            floor_i32(entity.position[0])?,
            floor_i32(entity.position[1])?,
            floor_i32(entity.position[2])?,
        );
        if region.is_some_and(|region| !region.contains(source)) {
            continue;
        }
        let transformed = transform_entity(entity, options.transform, BlockPos::ZERO);
        let content = BlockPos::new(
            floor_i32(transformed.position[0])?,
            floor_i32(transformed.position[1])?,
            floor_i32(transformed.position[2])?,
        );
        state.world.spawn_entity(transformed);
        state.include_content(content);
    }
    Ok(())
}

fn bits_for_palette(size: usize) -> usize {
    (usize::BITS - size.saturating_sub(1).leading_zeros()).max(4) as usize
}

fn floor_i32(value: f64) -> Result<i32, String> {
    let value = value.floor();
    if value < i32::MIN as f64 || value > i32::MAX as f64 {
        return Err("实体坐标超出 i32 范围".to_owned());
    }
    Ok(value as i32)
}

fn optional_list<'a>(
    map: &'a HashMap<String, Value>,
    key: &str,
) -> Result<Option<&'a Vec<Value>>, String> {
    match map.get(key) {
        Some(Value::List(value)) => Ok(Some(value)),
        Some(_) => Err(format!("字段 {key} 类型不是 list")),
        None => Ok(None),
    }
}

fn list<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a Vec<Value>, String> {
    optional_list(map, key)?.ok_or_else(|| format!("缺少字段 {key}"))
}

fn compound_value(value: &Value) -> Result<&HashMap<String, Value>, String> {
    match value {
        Value::Compound(value) => Ok(value),
        _ => Err("字段类型不是 compound".to_owned()),
    }
}

fn string<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a str, String> {
    match map.get(key) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(format!("缺少字符串字段 {key}")),
    }
}

fn integer(map: &HashMap<String, Value>, key: &str) -> Result<i32, String> {
    map.get(key)
        .and_then(value_i32)
        .ok_or_else(|| format!("缺少整数属性 {key}"))
}

fn properties(map: &HashMap<String, Value>) -> Result<BTreeMap<String, String>, String> {
    let Some(value) = map.get("Properties") else {
        return Ok(BTreeMap::new());
    };
    let Value::Compound(properties) = value else {
        return Err("方块 Properties 类型不是 compound".to_owned());
    };
    properties
        .iter()
        .map(|(key, value)| match value {
            Value::String(value) => Ok((key.clone(), value.clone())),
            _ => Err(format!("方块属性 {key} 不是字符串")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_palette_uses_four_bits_for_two_states() {
        assert_eq!(bits_for_palette(1), 4);
        assert_eq!(bits_for_palette(2), 4);
        assert_eq!(bits_for_palette(17), 5);
    }
}
