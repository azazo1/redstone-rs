use std::collections::{BTreeMap, HashMap};

use fastnbt::Value;
use redstone_core::{BlockPos, BlockStateId, SparseWorld};

use super::{
    LoadedStructure, StructureError, StructureStateResolver, StructureTransform,
    complete_properties, entity_from_nbt, nbt_compound_to_json, nbt_to_block_entity, resolve,
    transform_entity, transform_properties, transformed_bounds, update_bounds, value_i32,
};

const SPONGE_V1_DATA_VERSION: i32 = 1631;

pub(super) fn load<R: StructureStateResolver>(
    root: HashMap<String, Value>,
    transform: StructureTransform,
    resolver: &mut R,
) -> Result<LoadedStructure, StructureError> {
    let (schematic, version) = schematic_root(&root)?;
    let dimensions = dimensions(schematic)?;
    let volume = dimensions
        .iter()
        .try_fold(1usize, |volume, dimension| volume.checked_mul(*dimension))
        .ok_or_else(|| invalid("方块体积溢出"))?;
    let placement = placement(schematic, version)?;
    let finish = BlockPos::new(
        checked_add(placement.region_start.x, dimensions[0] - 1)?,
        checked_add(placement.region_start.y, dimensions[1] - 1)?,
        checked_add(placement.region_start.z, dimensions[2] - 1)?,
    );
    let (region_min, region_max) =
        transformed_bounds(transform, placement.region_start, finish);

    let block_container = if version == 3 {
        compound(schematic, "Blocks")?
    } else {
        schematic
    };
    let palette = decode_palette(block_container, version, transform, resolver)?;
    let block_data = byte_array(
        block_container,
        if version == 3 { "Data" } else { "BlockData" },
    )?;

    let mut world = SparseWorld::new(resolver.air_state());
    let mut counts = BTreeMap::new();
    let mut min = None;
    let mut max = None;
    let mut decoder = VarIntDecoder::new(block_data);
    for index in 0..volume {
        let palette_index = decoder
            .next()?
            .ok_or_else(|| invalid(format!("BlockData 仅包含 {index} 个方块, 期望 {volume} 个")))?;
        let (state, name) = palette
            .get(&palette_index)
            .ok_or(StructureError::InvalidPaletteIndex(palette_index as usize))?;
        if *state == resolver.air_state() {
            continue;
        }
        let local = position_from_index(index, dimensions, placement.region_start);
        let absolute = transform.apply(local);
        world.set_block(absolute, *state)?;
        update_bounds(&mut min, &mut max, absolute);
        *counts.entry(name.clone()).or_default() += 1;
    }
    if decoder.next()?.is_some() {
        return Err(invalid(format!(
            "BlockData 包含超过结构体积 {volume} 的方块"
        )));
    }

    let mut block_entity_nbt = BTreeMap::new();
    load_block_entities(
        block_container,
        version,
        dimensions,
        placement.region_start,
        transform,
        &mut world,
        &mut block_entity_nbt,
    )?;
    load_entities(
        schematic,
        version,
        placement,
        transform,
        &mut world,
    )?;

    Ok(LoadedStructure {
        world,
        min: min.unwrap_or(transform.origin),
        max: max.unwrap_or(transform.origin),
        region_min,
        region_max,
        format: "sponge_schematic".to_owned(),
        data_version: if version == 1 {
            Some(SPONGE_V1_DATA_VERSION)
        } else {
            schematic.get("DataVersion").and_then(value_i32)
        },
        block_counts: counts,
        block_entity_nbt,
    })
}

#[derive(Clone, Copy)]
struct Placement {
    region_start: BlockPos,
    file_origin: BlockPos,
}

fn schematic_root(
    root: &HashMap<String, Value>,
) -> Result<(&HashMap<String, Value>, i32), StructureError> {
    if let Some(Value::Compound(schematic)) = root.get("Schematic") {
        let version = integer(schematic, "Version")?;
        if version != 3 {
            return Err(invalid(format!(
                "Schematic 包装节点仅支持版本 3, 收到版本 {version}"
            )));
        }
        return Ok((schematic, version));
    }
    let version = integer(root, "Version")?;
    if !matches!(version, 1 | 2) {
        return Err(invalid(format!("不支持版本 {version}")));
    }
    Ok((root, version))
}

fn dimensions(schematic: &HashMap<String, Value>) -> Result<[usize; 3], StructureError> {
    let dimensions = [
        unsigned_short(schematic, "Width")?,
        unsigned_short(schematic, "Height")?,
        unsigned_short(schematic, "Length")?,
    ];
    if dimensions.contains(&0) {
        return Err(invalid("结构维度不能为 0"));
    }
    Ok(dimensions)
}

fn placement(
    schematic: &HashMap<String, Value>,
    version: i32,
) -> Result<Placement, StructureError> {
    let offset = optional_int_array_pos(schematic, "Offset")?.unwrap_or(BlockPos::ZERO);
    if version == 3 {
        return Ok(Placement {
            region_start: offset,
            file_origin: BlockPos::ZERO,
        });
    }
    let region_start = match schematic.get("Metadata") {
        Some(Value::Compound(metadata)) if metadata.contains_key("WEOffsetX") => BlockPos::new(
            integer(metadata, "WEOffsetX")?,
            integer(metadata, "WEOffsetY")?,
            integer(metadata, "WEOffsetZ")?,
        ),
        _ => BlockPos::ZERO,
    };
    Ok(Placement {
        region_start,
        file_origin: BlockPos::new(
            checked_sub(offset.x, region_start.x)?,
            checked_sub(offset.y, region_start.y)?,
            checked_sub(offset.z, region_start.z)?,
        ),
    })
}

fn decode_palette<R: StructureStateResolver>(
    block_container: &HashMap<String, Value>,
    version: i32,
    transform: StructureTransform,
    resolver: &mut R,
) -> Result<HashMap<u32, (BlockStateId, String)>, StructureError> {
    let palette_tag = compound(block_container, "Palette")?;
    if version < 3
        && let Some(palette_max) = block_container.get("PaletteMax").and_then(value_i32)
        && usize::try_from(palette_max).ok() != Some(palette_tag.len())
    {
        return Err(invalid(format!(
            "PaletteMax 为 {palette_max}, 但 Palette 包含 {} 项",
            palette_tag.len()
        )));
    }
    let mut palette = HashMap::with_capacity(palette_tag.len());
    for (state_text, id) in palette_tag {
        let id = value_i32(id)
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| invalid(format!("Palette 项 {state_text} 的索引无效")))?;
        let (name, explicit_properties) = parse_block_state(state_text)?;
        let properties = complete_properties(resolver, name, &explicit_properties)?;
        let properties = transform_properties(properties, transform);
        let state = resolve(resolver, name, &properties)?;
        if palette.insert(id, (state, name.to_owned())).is_some() {
            return Err(invalid(format!("Palette 索引 {id} 重复")));
        }
    }
    if palette.is_empty() {
        return Err(invalid("Palette 不能为空"));
    }
    Ok(palette)
}

fn parse_block_state(
    state: &str,
) -> Result<(&str, BTreeMap<String, String>), StructureError> {
    let Some((name, properties)) = state.split_once('[') else {
        if state.is_empty() {
            return Err(invalid("Palette 中存在空方块状态"));
        }
        return Ok((state, BTreeMap::new()));
    };
    let Some(properties) = properties.strip_suffix(']') else {
        return Err(invalid(format!("方块状态缺少右方括号: {state}")));
    };
    if name.is_empty() || properties.is_empty() {
        return Err(invalid(format!("方块状态语法无效: {state}")));
    }
    let mut parsed = BTreeMap::new();
    for property in properties.split(',') {
        let Some((key, value)) = property.split_once('=') else {
            return Err(invalid(format!("方块属性语法无效: {property}")));
        };
        if key.is_empty() || value.is_empty() || parsed.insert(key.to_owned(), value.to_owned()).is_some() {
            return Err(invalid(format!("方块属性无效或重复: {property}")));
        }
    }
    Ok((name, parsed))
}

fn load_block_entities(
    block_container: &HashMap<String, Value>,
    version: i32,
    dimensions: [usize; 3],
    region_start: BlockPos,
    transform: StructureTransform,
    world: &mut SparseWorld,
    block_entity_nbt: &mut BTreeMap<BlockPos, BTreeMap<String, serde_json::Value>>,
) -> Result<(), StructureError> {
    let entries = if version < 3 && !block_container.contains_key("BlockEntities") {
        optional_list(block_container, "TileEntities")?
    } else {
        optional_list(block_container, "BlockEntities")?
    };
    for entry in entries.into_iter().flatten() {
        let entry = compound_value(entry)?;
        let relative = int_array_pos(entry, "Pos")?;
        if relative.x < 0
            || relative.y < 0
            || relative.z < 0
            || relative.x as usize >= dimensions[0]
            || relative.y as usize >= dimensions[1]
            || relative.z as usize >= dimensions[2]
        {
            return Err(invalid(format!("方块实体坐标超出结构范围: {relative:?}")));
        }
        let id = string_any(entry, &["Id", "id"])?;
        let mut nbt = if version == 3 {
            entry
                .get("Data")
                .and_then(|value| match value {
                    Value::Compound(value) => Some(value.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        } else {
            entry.clone()
        };
        nbt.remove("Id");
        nbt.remove("Pos");
        nbt.insert("id".to_owned(), Value::String(id.to_owned()));
        let local = region_start.offset(relative.x, relative.y, relative.z);
        let absolute = transform.apply(local);
        nbt.insert("x".to_owned(), Value::Int(absolute.x));
        nbt.insert("y".to_owned(), Value::Int(absolute.y));
        nbt.insert("z".to_owned(), Value::Int(absolute.z));
        block_entity_nbt.insert(absolute, nbt_compound_to_json(&nbt));
        world.set_block_entity(absolute, nbt_to_block_entity(&nbt));
    }
    Ok(())
}

fn load_entities(
    schematic: &HashMap<String, Value>,
    version: i32,
    placement: Placement,
    transform: StructureTransform,
    world: &mut SparseWorld,
) -> Result<(), StructureError> {
    let Some(entries) = optional_list(schematic, "Entities")? else {
        return Ok(());
    };
    for entry in entries {
        let entry = compound_value(entry)?;
        let id = string_any(entry, &["Id", "id"])?;
        let mut nbt = if version == 3 {
            entry
                .get("Data")
                .and_then(|value| match value {
                    Value::Compound(value) => Some(value.clone()),
                    _ => None,
                })
                .unwrap_or_default()
        } else {
            entry.clone()
        };
        nbt.remove("Id");
        nbt.insert("id".to_owned(), Value::String(id.to_owned()));
        if let Some(position) = entry.get("Pos") {
            nbt.insert("Pos".to_owned(), position.clone());
        }
        if let Some(mut entity) = entity_from_nbt(&nbt)? {
            if version == 3 {
                entity.position[0] += placement.region_start.x as f64;
                entity.position[1] += placement.region_start.y as f64;
                entity.position[2] += placement.region_start.z as f64;
            } else {
                entity.position[0] -= placement.file_origin.x as f64;
                entity.position[1] -= placement.file_origin.y as f64;
                entity.position[2] -= placement.file_origin.z as f64;
            }
            world.spawn_entity(transform_entity(entity, transform, BlockPos::ZERO));
        }
    }
    Ok(())
}

fn position_from_index(
    index: usize,
    dimensions: [usize; 3],
    region_start: BlockPos,
) -> BlockPos {
    let x = index % dimensions[0];
    let z = (index / dimensions[0]) % dimensions[2];
    let y = index / (dimensions[0] * dimensions[2]);
    region_start.offset(x as i32, y as i32, z as i32)
}

struct VarIntDecoder<'a> {
    bytes: &'a [i8],
    offset: usize,
}

impl<'a> VarIntDecoder<'a> {
    fn new(bytes: &'a [i8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn next(&mut self) -> Result<Option<u32>, StructureError> {
        if self.offset == self.bytes.len() {
            return Ok(None);
        }
        let mut value = 0u32;
        for byte_index in 0..5 {
            let byte = *self
                .bytes
                .get(self.offset)
                .ok_or_else(|| invalid("BlockData 中存在不完整的 VarInt"))?
                as u8;
            self.offset += 1;
            if byte_index == 4 && byte & 0xf0 != 0 {
                return Err(invalid("BlockData 中的 VarInt 超出 32 位范围"));
            }
            value |= u32::from(byte & 0x7f) << (byte_index * 7);
            if byte & 0x80 == 0 {
                return Ok(Some(value));
            }
        }
        Err(invalid("BlockData 中的 VarInt 超过 5 字节"))
    }
}

fn checked_add(value: i32, increment: usize) -> Result<i32, StructureError> {
    value
        .checked_add(increment as i32)
        .ok_or_else(|| invalid("结构边界坐标溢出"))
}

fn checked_sub(value: i32, subtrahend: i32) -> Result<i32, StructureError> {
    value
        .checked_sub(subtrahend)
        .ok_or_else(|| invalid("结构原点坐标溢出"))
}

fn unsigned_short(
    map: &HashMap<String, Value>,
    key: &str,
) -> Result<usize, StructureError> {
    match map.get(key) {
        Some(Value::Short(value)) => Ok((*value as u16) as usize),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn integer(map: &HashMap<String, Value>, key: &str) -> Result<i32, StructureError> {
    map.get(key)
        .and_then(value_i32)
        .ok_or_else(|| StructureError::MissingField(key.to_owned()))
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

fn optional_list<'a>(
    map: &'a HashMap<String, Value>,
    key: &str,
) -> Result<Option<&'a Vec<Value>>, StructureError> {
    match map.get(key) {
        Some(Value::List(value)) => Ok(Some(value)),
        Some(_) => Err(StructureError::InvalidType("list")),
        None => Ok(None),
    }
}

fn byte_array<'a>(
    map: &'a HashMap<String, Value>,
    key: &str,
) -> Result<&'a [i8], StructureError> {
    match map.get(key) {
        Some(Value::ByteArray(value)) => Ok(value),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn optional_int_array_pos(
    map: &HashMap<String, Value>,
    key: &str,
) -> Result<Option<BlockPos>, StructureError> {
    match map.get(key) {
        Some(_) => int_array_pos(map, key).map(Some),
        None => Ok(None),
    }
}

fn int_array_pos(
    map: &HashMap<String, Value>,
    key: &str,
) -> Result<BlockPos, StructureError> {
    match map.get(key) {
        Some(Value::IntArray(value)) if value.len() == 3 => {
            Ok(BlockPos::new(value[0], value[1], value[2]))
        }
        Some(Value::IntArray(_)) => Err(invalid(format!("{key} 必须包含 3 个整数"))),
        _ => Err(StructureError::MissingField(key.to_owned())),
    }
}

fn string_any<'a>(
    map: &'a HashMap<String, Value>,
    keys: &[&str],
) -> Result<&'a str, StructureError> {
    keys.iter()
        .find_map(|key| match map.get(*key) {
            Some(Value::String(value)) => Some(value.as_str()),
            _ => None,
        })
        .ok_or_else(|| StructureError::MissingField(keys.join(" 或 ")))
}

fn invalid(message: impl Into<String>) -> StructureError {
    StructureError::InvalidSpongeSchematic(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_can_cross_byte_boundaries() {
        let mut decoder = VarIntDecoder::new(&[0, -84, 2, -1, -1, -1, -1, 7]);
        assert_eq!(decoder.next().unwrap(), Some(0));
        assert_eq!(decoder.next().unwrap(), Some(300));
        assert_eq!(decoder.next().unwrap(), Some(i32::MAX as u32));
        assert_eq!(decoder.next().unwrap(), None);
    }

    #[test]
    fn block_state_parser_separates_properties() {
        let (name, properties) =
            parse_block_state("minecraft:observer[facing=north,powered=false]").unwrap();
        assert_eq!(name, "minecraft:observer");
        assert_eq!(properties["facing"], "north");
        assert_eq!(properties["powered"], "false");
    }
}
