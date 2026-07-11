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
    let palette = list(&root, "palette")?;
    let mut states = Vec::new();
    let mut names = Vec::new();
    for entry in palette {
        let entry = compound_value(entry)?;
        let name = string(entry, "Name")?;
        let properties = transform_properties(optional_properties(entry, "Properties")?, transform);
        states.push(resolve(resolver, name, &properties)?);
        names.push(name.to_owned());
    }

    let size = int_list(&root, "size")?;
    let (region_min, region_max) = if size.iter().all(|dimension| *dimension > 0) {
        transformed_bounds(
            transform,
            BlockPos::ZERO,
            BlockPos::new(size[0] - 1, size[1] - 1, size[2] - 1),
        )
    } else {
        (transform.origin, transform.origin)
    };
    let mut world = SparseWorld::new(resolver.air_state());
    let mut counts = BTreeMap::new();
    let mut block_entity_nbt = BTreeMap::new();
    let mut min = None;
    let mut max = None;
    for block in list(&root, "blocks")? {
        let block = compound_value(block)?;
        let pos = int_list(block, "pos")?;
        let palette_index = usize::try_from(integer(block, "state")?)
            .map_err(|_| StructureError::InvalidPaletteIndex(usize::MAX))?;
        let state = *states
            .get(palette_index)
            .ok_or(StructureError::InvalidPaletteIndex(palette_index))?;
        let absolute = transform.apply(BlockPos::new(pos[0], pos[1], pos[2]));
        world.set_block(absolute, state)?;
        update_bounds(&mut min, &mut max, absolute);
        *counts.entry(names[palette_index].clone()).or_default() += 1;
        if let Some(Value::Compound(nbt)) = block.get("nbt") {
            block_entity_nbt.insert(absolute, nbt_compound_to_json(nbt));
            world.set_block_entity(absolute, nbt_to_block_entity(nbt));
        }
    }
    if let Ok(entities) = list(&root, "entities") {
        for entity in entities {
            let entity = compound_value(entity)?;
            if let Some(local) = entity_from_nbt(entity)? {
                world.spawn_entity(transform_entity(local, transform, BlockPos::ZERO));
            }
        }
    }
    Ok(LoadedStructure {
        world,
        min: min.unwrap_or(transform.origin),
        max: max.unwrap_or(region_max),
        region_min,
        region_max,
        format: "vanilla_structure".to_owned(),
        data_version: root.get("DataVersion").and_then(value_i32),
        block_counts: counts,
        block_entity_nbt,
    })
}

fn list<'a>(map: &'a HashMap<String, Value>, key: &str) -> Result<&'a Vec<Value>, StructureError> {
    match map.get(key) {
        Some(Value::List(value)) => Ok(value),
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

fn int_list(map: &HashMap<String, Value>, key: &str) -> Result<[i32; 3], StructureError> {
    let value = list(map, key)?;
    if value.len() != 3 {
        return Err(StructureError::InvalidType("3 element integer list"));
    }
    Ok([
        value_i32(&value[0]).ok_or(StructureError::InvalidType("integer"))?,
        value_i32(&value[1]).ok_or(StructureError::InvalidType("integer"))?,
        value_i32(&value[2]).ok_or(StructureError::InvalidType("integer"))?,
    ])
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
