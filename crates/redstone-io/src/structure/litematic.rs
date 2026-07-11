use std::collections::{BTreeMap, HashMap};

use fastnbt::Value;
use redstone_core::{BlockPos, SparseWorld};

use super::{
    LoadedStructure, StructureError, StructureStateResolver, StructureTransform, entity_from_nbt,
    nbt_compound_to_json, nbt_to_block_entity, resolve, transform_entity, transform_properties,
    transformed_bounds, update_bounds, value_i32,
};

pub(super) fn load<R: StructureStateResolver>(
    root: HashMap<String, Value>,
    transform: StructureTransform,
    resolver: &mut R,
) -> Result<LoadedStructure, StructureError> {
    let regions = compound(&root, "Regions")?;
    let mut world = SparseWorld::new(resolver.air_state());
    let mut counts = BTreeMap::new();
    let mut block_entity_nbt = BTreeMap::new();
    let mut min = None;
    let mut max = None;
    let mut region_min = None;
    let mut region_max = None;

    let mut region_entries = regions.iter().collect::<Vec<_>>();
    region_entries.sort_by(|(left_name, left), (right_name, right)| {
        let left_pos = compound_value(left)
            .and_then(|region| xyz_compound(region, "Position"))
            .unwrap_or(BlockPos::ZERO);
        let right_pos = compound_value(right)
            .and_then(|region| xyz_compound(region, "Position"))
            .unwrap_or(BlockPos::ZERO);
        left_pos
            .cmp(&right_pos)
            .then_with(|| left_name.cmp(right_name))
    });
    for (_region_name, region_value) in region_entries {
        let region = compound_value(region_value)?;
        let region_position = xyz_compound(region, "Position")?;
        let size = xyz_compound(region, "Size")?;
        let end = region_position.offset(
            relative_end(size.x),
            relative_end(size.y),
            relative_end(size.z),
        );
        let start = BlockPos::new(
            region_position.x.min(end.x),
            region_position.y.min(end.y),
            region_position.z.min(end.z),
        );
        let finish = BlockPos::new(
            region_position.x.max(end.x),
            region_position.y.max(end.y),
            region_position.z.max(end.z),
        );
        let dimensions = [
            size.x.unsigned_abs(),
            size.y.unsigned_abs(),
            size.z.unsigned_abs(),
        ];
        if dimensions.contains(&0) {
            continue;
        }
        let (transformed_min, transformed_max) = transformed_bounds(transform, start, finish);
        update_bounds(&mut region_min, &mut region_max, transformed_min);
        update_bounds(&mut region_min, &mut region_max, transformed_max);
        let palette = list(region, "BlockStatePalette")?;
        let mut states = Vec::new();
        let mut names = Vec::new();
        for entry in palette {
            let entry = compound_value(entry)?;
            let name = string(entry, "Name")?;
            let properties =
                transform_properties(optional_properties(entry, "Properties")?, transform);
            states.push(resolve(resolver, name, &properties)?);
            names.push(name.to_owned());
        }
        let bits = bits_for_palette(states.len());
        let longs = long_array(region, "BlockStates")?;
        let volume = dimensions[0] as usize * dimensions[1] as usize * dimensions[2] as usize;
        for index in 0..volume {
            let palette_index = unpack_palette_index(longs, index, bits);
            let state = *states
                .get(palette_index)
                .ok_or(StructureError::InvalidPaletteIndex(palette_index))?;
            if state == resolver.air_state() {
                continue;
            }
            let x = index % dimensions[0] as usize;
            let z = (index / dimensions[0] as usize) % dimensions[2] as usize;
            let y = index / (dimensions[0] as usize * dimensions[2] as usize);
            let absolute = transform.apply(BlockPos::new(
                start.x + x as i32,
                start.y + y as i32,
                start.z + z as i32,
            ));
            world.set_block(absolute, state)?;
            *counts.entry(names[palette_index].clone()).or_default() += 1;
            update_bounds(&mut min, &mut max, absolute);
        }
        if let Ok(block_entities) = list(region, "TileEntities") {
            for entry in block_entities {
                let entry = compound_value(entry)?;
                let x = integer(entry, "x")?;
                let y = integer(entry, "y")?;
                let z = integer(entry, "z")?;
                let absolute =
                    transform.apply(BlockPos::new(start.x + x, start.y + y, start.z + z));
                block_entity_nbt.insert(absolute, nbt_compound_to_json(entry));
                world.set_block_entity(absolute, nbt_to_block_entity(entry));
            }
        }
        if let Ok(entities) = list(region, "Entities") {
            for entry in entities {
                let entry = compound_value(entry)?;
                if let Some(local) = entity_from_nbt(entry)? {
                    world.spawn_entity(transform_entity(local, transform, region_position));
                }
            }
        }
    }
    Ok(LoadedStructure {
        world,
        min: min.unwrap_or(transform.origin),
        max: max.unwrap_or(transform.origin),
        region_min: region_min.unwrap_or(transform.origin),
        region_max: region_max.unwrap_or(transform.origin),
        format: "litematic".to_owned(),
        data_version: root.get("MinecraftDataVersion").and_then(value_i32),
        block_counts: counts,
        block_entity_nbt,
    })
}

fn bits_for_palette(size: usize) -> usize {
    (usize::BITS - size.saturating_sub(1).leading_zeros()).max(2) as usize
}

fn relative_end(size: i32) -> i32 {
    if size >= 0 { size - 1 } else { size + 1 }
}

fn unpack_palette_index(longs: &[i64], index: usize, bits: usize) -> usize {
    let mask = (1u64 << bits) - 1;
    let bit_index = index * bits;
    let start_long = bit_index / 64;
    let start_offset = bit_index % 64;
    let mut value = (longs.get(start_long).copied().unwrap_or(0) as u64) >> start_offset;
    if start_offset + bits > 64 {
        value |= (longs.get(start_long + 1).copied().unwrap_or(0) as u64) << (64 - start_offset);
    }
    (value & mask) as usize
}

fn list<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a Vec<Value>, StructureError> {
    match map.get(key) {
        Some(Value::List(value)) => Ok(value),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn compound<'a>(
    map: &'a HashMap<String, Value>,
    key: &str,
) -> Result<&'a HashMap<String, Value>, StructureError> {
    match map.get(key) {
        Some(Value::Compound(value)) => Ok(value),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn compound_value(value: &Value) -> Result<&HashMap<String, Value>, StructureError> {
    match value {
        Value::Compound(value) => Ok(value),
        _ => Err(StructureError::InvalidType("compound")),
    }
}

fn string<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a str, StructureError> {
    match map.get(key) {
        Some(Value::String(value)) => Ok(value),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn integer(map: &HashMap<String, Value>, key: &str) -> Result<i32, StructureError> {
    map.get(key)
        .and_then(value_i32)
        .ok_or_else(|| StructureError::MissingField(key.to_owned()))
}

fn xyz_compound(map: &HashMap<String, Value>, key: &str) -> Result<BlockPos, StructureError> {
    let value = compound(map, key)?;
    Ok(BlockPos::new(
        integer(value, "x")?,
        integer(value, "y")?,
        integer(value, "z")?,
    ))
}

fn optional_properties(
    map: &HashMap<String, Value>,
    key: &str,
) -> Result<BTreeMap<String, String>, StructureError> {
    let Some(value) = map.get(key) else {
        return Ok(BTreeMap::new());
    };
    let Value::Compound(properties) = value else {
        return Err(StructureError::InvalidType("properties"));
    };
    properties
        .iter()
        .map(|(key, value)| match value {
            Value::String(value) => Ok((key.clone(), value.clone())),
            _ => Err(StructureError::InvalidType("string property")),
        })
        .collect()
}

fn long_array<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a [i64], StructureError> {
    match map.get(key) {
        Some(Value::LongArray(value)) => Ok(value),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_palette_values_can_cross_long_boundaries() {
        let bits = 5;
        let values = [3usize, 7, 17, 31, 1, 9, 12, 25, 4, 30, 2, 15, 28, 6];
        let mut longs = vec![0u64; (values.len() * bits).div_ceil(64)];
        for (index, value) in values.iter().copied().enumerate() {
            let bit_index = index * bits;
            let start = bit_index / 64;
            let offset = bit_index % 64;
            longs[start] |= (value as u64) << offset;
            if offset + bits > 64 {
                longs[start + 1] |= (value as u64) >> (64 - offset);
            }
        }
        let longs = longs
            .into_iter()
            .map(|value| value as i64)
            .collect::<Vec<_>>();
        for (index, expected) in values.into_iter().enumerate() {
            assert_eq!(unpack_palette_index(&longs, index, bits), expected);
        }
    }

    #[test]
    fn relative_end_matches_negative_size_contract() {
        assert_eq!(relative_end(3), 2);
        assert_eq!(relative_end(-3), -2);
    }
}
