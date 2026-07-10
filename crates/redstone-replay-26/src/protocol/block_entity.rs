use std::collections::BTreeMap;

use redstone_core::{BlockEntityData, BlockPos};
use serde_json::Value;
use thiserror::Error;

use super::buf::PacketBuf;

const BLOCK_ENTITY_TYPES: &str = include_str!("../../data/block-entity-types.txt");

pub(crate) fn write_chunk_block_entity(
    output: &mut PacketBuf,
    pos: BlockPos,
    data: &BlockEntityData,
) -> Result<(), BlockEntityEncodeError> {
    let xz = ((pos.x.rem_euclid(16) as u8) << 4) | pos.z.rem_euclid(16) as u8;
    output.write_u8(xz);
    output.write_u16(pos.y as u16);
    output.write_var_i32(block_entity_type_id(&data.kind)?);
    output.write_bytes(&network_nbt(data)?);
    Ok(())
}

pub(crate) fn block_entity_data(
    pos: BlockPos,
    data: &BlockEntityData,
) -> Result<Vec<u8>, BlockEntityEncodeError> {
    let mut output = PacketBuf::new();
    output.write_block_pos(pos);
    output.write_var_i32(block_entity_type_id(&data.kind)?);
    output.write_bytes(&network_nbt(data)?);
    Ok(output.into_inner())
}

fn block_entity_type_id(kind: &str) -> Result<i32, BlockEntityEncodeError> {
    let network_kind = match kind {
        "minecraft:moving_piston" => "minecraft:piston",
        kind if kind.ends_with("_shulker_box") => "minecraft:shulker_box",
        kind => kind,
    };
    BLOCK_ENTITY_TYPES
        .lines()
        .position(|entry| entry == network_kind)
        .and_then(|id| i32::try_from(id).ok())
        .ok_or_else(|| BlockEntityEncodeError::UnknownType(kind.to_owned()))
}

fn network_nbt(data: &BlockEntityData) -> Result<Vec<u8>, BlockEntityEncodeError> {
    let compound = if data.kind == "minecraft:moving_piston" {
        moving_piston_nbt(data)?
    } else {
        generic_block_entity_nbt(data)?
    };
    let mut output = Vec::new();
    let root = NbtValue::Compound(compound);
    output.push(root.tag_id());
    root.write_payload(&mut output)?;
    Ok(output)
}

fn moving_piston_nbt(
    data: &BlockEntityData,
) -> Result<BTreeMap<String, NbtValue>, BlockEntityEncodeError> {
    let name = data
        .fields
        .get("moved_state_name")
        .and_then(Value::as_str)
        .ok_or(BlockEntityEncodeError::MissingMovingPistonField(
            "moved_state_name",
        ))?;
    let properties = data
        .fields
        .get("moved_state_properties")
        .and_then(Value::as_object)
        .map(|properties| {
            properties
                .iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.clone(), NbtValue::String(value.to_owned())))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut block_state = BTreeMap::from([(
        "Name".to_owned(),
        NbtValue::String(name.to_owned()),
    )]);
    if !properties.is_empty() {
        block_state.insert("Properties".to_owned(), NbtValue::Compound(properties));
    }
    let direction = data
        .fields
        .get("direction")
        .and_then(Value::as_str)
        .and_then(direction_id)
        .ok_or(BlockEntityEncodeError::MissingMovingPistonField(
            "direction",
        ))?;
    let extending = data
        .fields
        .get("extending")
        .and_then(Value::as_bool)
        .ok_or(BlockEntityEncodeError::MissingMovingPistonField(
            "extending",
        ))?;
    let source = data
        .fields
        .get("source")
        .and_then(Value::as_bool)
        .ok_or(BlockEntityEncodeError::MissingMovingPistonField("source"))?;
    let progress = data
        .fields
        .get("progress")
        .and_then(Value::as_f64)
        .unwrap_or(0.0) as f32;
    Ok(BTreeMap::from([
        ("blockState".to_owned(), NbtValue::Compound(block_state)),
        ("extending".to_owned(), NbtValue::Byte(i8::from(extending))),
        ("facing".to_owned(), NbtValue::Int(direction)),
        ("progress".to_owned(), NbtValue::Float(progress)),
        ("source".to_owned(), NbtValue::Byte(i8::from(source))),
    ]))
}

fn generic_block_entity_nbt(
    data: &BlockEntityData,
) -> Result<BTreeMap<String, NbtValue>, BlockEntityEncodeError> {
    let is_container = is_container_kind(&data.kind);
    let mut output = BTreeMap::new();
    for (key, value) in &data.fields {
        if is_internal_field(key) || (is_container && key == "Items") {
            continue;
        }
        if let Some(value) = json_to_nbt(value)? {
            output.insert(key.clone(), value);
        }
    }
    if is_container {
        output.insert("Items".to_owned(), container_items(data)?);
        if let Some(cooldown) = data.fields.get("cooldown").and_then(Value::as_i64) {
            output.insert("TransferCooldown".to_owned(), NbtValue::Int(clamp_i32(cooldown)));
        }
    }
    if matches!(data.kind.as_str(), "minecraft:sign" | "minecraft:hanging_sign") {
        add_sign_text_defaults(data, &mut output)?;
    }
    Ok(output)
}

fn container_items(data: &BlockEntityData) -> Result<NbtValue, BlockEntityEncodeError> {
    let original = data
        .fields
        .get("Items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let slot = entry
                .get("Slot")
                .or_else(|| entry.get("slot"))
                .and_then(Value::as_i64)?;
            Some((slot, entry))
        })
        .collect::<BTreeMap<_, _>>();
    let mut inventory = data
        .fields
        .get("inventory")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if inventory.is_empty() {
        inventory.extend(original.iter().filter_map(|(slot, entry)| {
            let item_id = entry.get("id").and_then(Value::as_str)?;
            let count = entry
                .get("count")
                .or_else(|| entry.get("Count"))
                .and_then(Value::as_i64)?;
            if count <= 0 {
                return None;
            }
            let mut item = serde_json::Map::from_iter([
                ("slot".to_owned(), Value::from(*slot)),
                ("item_id".to_owned(), Value::String(item_id.to_owned())),
                ("count".to_owned(), Value::from(count)),
            ]);
            if let Some(components) = entry.get("components") {
                item.insert("components".to_owned(), components.clone());
            }
            Some(Value::Object(item))
        }));
    }
    if inventory.is_empty()
        && let (Some(item_id), Some(count)) = (
            data.fields.get("item_id").and_then(Value::as_str),
            data.fields.get("item_count").and_then(Value::as_i64),
        )
        && count > 0
    {
        inventory.push(serde_json::json!({
            "slot": 0,
            "item_id": item_id,
            "count": count,
        }));
    }
    inventory.sort_by_key(|entry| entry.get("slot").and_then(Value::as_i64).unwrap_or(0));
    let mut items = Vec::new();
    for entry in inventory {
        let Some(entry) = entry.as_object() else {
            continue;
        };
        let Some(slot) = entry.get("slot").and_then(Value::as_i64) else {
            continue;
        };
        let Some(item_id) = entry.get("item_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(count) = entry.get("count").and_then(Value::as_i64) else {
            continue;
        };
        if count <= 0 {
            continue;
        }
        let mut item = BTreeMap::from([
            ("Slot".to_owned(), NbtValue::Byte(slot.clamp(0, 127) as i8)),
            ("count".to_owned(), NbtValue::Int(clamp_i32(count))),
            ("id".to_owned(), NbtValue::String(item_id.to_owned())),
        ]);
        let original_components = original.get(&slot).and_then(|entry| {
            (entry.get("id").and_then(Value::as_str) == Some(item_id))
                .then(|| entry.get("components"))
                .flatten()
        });
        if let Some(components) = entry.get("components").or(original_components)
            && let Some(components) = json_to_nbt(components)?
        {
            item.insert("components".to_owned(), components);
        }
        items.push(NbtValue::Compound(item));
    }
    Ok(NbtValue::List(items))
}

fn add_sign_text_defaults(
    data: &BlockEntityData,
    output: &mut BTreeMap<String, NbtValue>,
) -> Result<(), BlockEntityEncodeError> {
    if !output.contains_key("front_text") {
        let messages = (1..=4)
            .map(|index| {
                data.fields
                    .get(&format!("Text{index}"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned()
            })
            .collect::<Vec<_>>();
        output.insert(
            "front_text".to_owned(),
            sign_text(
                messages,
                data.fields
                    .get("Color")
                    .and_then(Value::as_str)
                    .unwrap_or("black"),
                data.fields
                    .get("GlowingText")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            ),
        );
    }
    if !output.contains_key("back_text") {
        output.insert(
            "back_text".to_owned(),
            sign_text(vec![String::new(); 4], "black", false),
        );
    }
    Ok(())
}

fn sign_text(messages: Vec<String>, color: &str, glowing: bool) -> NbtValue {
    let messages = messages
        .into_iter()
        .map(NbtValue::String)
        .collect::<Vec<_>>();
    NbtValue::Compound(BTreeMap::from([
        ("color".to_owned(), NbtValue::String(color.to_owned())),
        (
            "filtered_messages".to_owned(),
            NbtValue::List(messages.clone()),
        ),
        (
            "has_glowing_text".to_owned(),
            NbtValue::Byte(i8::from(glowing)),
        ),
        ("messages".to_owned(), NbtValue::List(messages)),
    ]))
}

fn json_to_nbt(value: &Value) -> Result<Option<NbtValue>, BlockEntityEncodeError> {
    Ok(match value {
        Value::Null => None,
        Value::Bool(value) => Some(NbtValue::Byte(i8::from(*value))),
        Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Some(if let Ok(value) = i32::try_from(value) {
                    NbtValue::Int(value)
                } else {
                    NbtValue::Long(value)
                })
            } else if let Some(value) = value.as_u64() {
                Some(if let Ok(value) = i32::try_from(value) {
                    NbtValue::Int(value)
                } else {
                    NbtValue::Long(i64::try_from(value).unwrap_or(i64::MAX))
                })
            } else {
                value.as_f64().map(NbtValue::Double)
            }
        }
        Value::String(value) => Some(NbtValue::String(value.clone())),
        Value::Array(values) => Some(NbtValue::List(
            values
                .iter()
                .filter_map(|value| json_to_nbt(value).transpose())
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Value::Object(values) => Some(NbtValue::Compound(
            values
                .iter()
                .filter_map(|(key, value)| {
                    json_to_nbt(value)
                        .transpose()
                        .map(|value| value.map(|value| (key.clone(), value)))
                })
                .collect::<Result<BTreeMap<_, _>, _>>()?,
        )),
    })
}

fn is_container_kind(kind: &str) -> bool {
    kind.ends_with("_shulker_box")
        || matches!(
            kind,
            "minecraft:barrel"
                | "minecraft:blast_furnace"
                | "minecraft:brewing_stand"
                | "minecraft:chest"
                | "minecraft:crafter"
                | "minecraft:dispenser"
                | "minecraft:dropper"
                | "minecraft:furnace"
                | "minecraft:hopper"
                | "minecraft:shulker_box"
                | "minecraft:smoker"
                | "minecraft:trapped_chest"
        )
}

fn is_internal_field(key: &str) -> bool {
    matches!(
        key,
        "capacity"
            | "comparator_output"
            | "cooldown"
            | "inventory"
            | "item_count"
            | "item_id"
            | "last_open_count"
            | "moved_block_entity"
            | "moved_state"
            | "moved_state_name"
            | "moved_state_properties"
            | "open_count"
            | "output_count"
            | "output_item_id"
            | "settle_tick"
            | "slot_count"
    )
}

fn direction_id(direction: &str) -> Option<i32> {
    Some(match direction {
        "down" => 0,
        "up" => 1,
        "north" => 2,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => return None,
    })
}

fn clamp_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

#[derive(Clone, Debug, PartialEq)]
enum NbtValue {
    Byte(i8),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    List(Vec<NbtValue>),
    Compound(BTreeMap<String, NbtValue>),
}

impl NbtValue {
    fn tag_id(&self) -> u8 {
        match self {
            Self::Byte(_) => 1,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::String(_) => 8,
            Self::List(_) => 9,
            Self::Compound(_) => 10,
        }
    }

    fn write_payload(&self, output: &mut Vec<u8>) -> Result<(), BlockEntityEncodeError> {
        match self {
            Self::Byte(value) => output.push(*value as u8),
            Self::Int(value) => output.extend_from_slice(&value.to_be_bytes()),
            Self::Long(value) => output.extend_from_slice(&value.to_be_bytes()),
            Self::Float(value) => output.extend_from_slice(&value.to_be_bytes()),
            Self::Double(value) => output.extend_from_slice(&value.to_be_bytes()),
            Self::String(value) => write_nbt_string(output, value)?,
            Self::List(values) => {
                let element_type = values.first().map_or(0, NbtValue::tag_id);
                if values.iter().any(|value| value.tag_id() != element_type) {
                    return Err(BlockEntityEncodeError::MixedListTypes);
                }
                output.push(element_type);
                output.extend_from_slice(
                    &i32::try_from(values.len())
                        .map_err(|_| BlockEntityEncodeError::NbtTooLarge)?
                        .to_be_bytes(),
                );
                for value in values {
                    value.write_payload(output)?;
                }
            }
            Self::Compound(values) => {
                for (name, value) in values {
                    output.push(value.tag_id());
                    write_nbt_string(output, name)?;
                    value.write_payload(output)?;
                }
                output.push(0);
            }
        }
        Ok(())
    }
}

fn write_nbt_string(
    output: &mut Vec<u8>,
    value: &str,
) -> Result<(), BlockEntityEncodeError> {
    let length = u16::try_from(value.len()).map_err(|_| BlockEntityEncodeError::NbtTooLarge)?;
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

#[derive(Debug, Error)]
pub(crate) enum BlockEntityEncodeError {
    #[error("未知的 Minecraft 26.1.2 方块实体类型: {0}")]
    UnknownType(String),
    #[error("moving_piston 缺少内部字段: {0}")]
    MissingMovingPistonField(&'static str),
    #[error("NBT 列表包含不同标签类型")]
    MixedListTypes,
    #[error("NBT 数据过大")]
    NbtTooLarge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_entity_registry_ids_match_protocol_report() {
        assert_eq!(block_entity_type_id("minecraft:sign").unwrap(), 7);
        assert_eq!(block_entity_type_id("minecraft:moving_piston").unwrap(), 11);
        assert_eq!(block_entity_type_id("minecraft:hopper").unwrap(), 18);
        assert_eq!(block_entity_type_id("minecraft:barrel").unwrap(), 27);
    }

    #[test]
    fn moving_piston_contains_vanilla_animation_fields() {
        let data = BlockEntityData {
            kind: "minecraft:moving_piston".to_owned(),
            fields: BTreeMap::from([
                ("direction".to_owned(), Value::String("east".to_owned())),
                ("extending".to_owned(), Value::Bool(true)),
                (
                    "moved_state_name".to_owned(),
                    Value::String("minecraft:orange_wool".to_owned()),
                ),
                (
                    "moved_state_properties".to_owned(),
                    serde_json::json!({}),
                ),
                ("source".to_owned(), Value::Bool(false)),
            ]),
        };
        let encoded = network_nbt(&data).unwrap();
        assert_eq!(encoded[0], 10);
        assert!(encoded.windows("blockState".len()).any(|value| value == b"blockState"));
        assert!(encoded.windows("progress".len()).any(|value| value == b"progress"));
        assert!(encoded.windows("orange_wool".len()).any(|value| value == b"orange_wool"));
    }

    #[test]
    fn sign_and_container_nbt_keep_visible_contents() {
        let sign = BlockEntityData {
            kind: "minecraft:sign".to_owned(),
            fields: BTreeMap::from([(
                "front_text".to_owned(),
                serde_json::json!({
                    "messages": ["one", "two", "three", "four"],
                    "filtered_messages": ["one", "two", "three", "four"],
                    "color": "black",
                    "has_glowing_text": false,
                }),
            )]),
        };
        let sign = network_nbt(&sign).unwrap();
        assert!(sign.windows("front_text".len()).any(|value| value == b"front_text"));
        assert!(sign.windows("back_text".len()).any(|value| value == b"back_text"));

        let container = BlockEntityData {
            kind: "minecraft:hopper".to_owned(),
            fields: BTreeMap::from([(
                "inventory".to_owned(),
                serde_json::json!([{
                    "slot": 2,
                    "item_id": "minecraft:redstone",
                    "count": 12,
                }]),
            )]),
        };
        let container = network_nbt(&container).unwrap();
        assert!(container.windows("Items".len()).any(|value| value == b"Items"));
        assert!(container.windows("redstone".len()).any(|value| value == b"redstone"));
    }

    #[test]
    fn container_nbt_accepts_raw_items_and_does_not_reuse_mismatched_components() {
        let raw = BlockEntityData {
            kind: "minecraft:hopper".to_owned(),
            fields: BTreeMap::from([(
                "Items".to_owned(),
                serde_json::json!([{
                    "Slot": 0,
                    "id": "minecraft:redstone",
                    "count": 2,
                    "components": {
                        "minecraft:custom_name": "{\"text\":\"Signal\"}",
                    },
                }]),
            )]),
        };
        let raw = network_nbt(&raw).unwrap();
        assert!(raw.windows("redstone".len()).any(|value| value == b"redstone"));
        assert!(raw.windows("custom_name".len()).any(|value| value == b"custom_name"));

        let replaced = BlockEntityData {
            kind: "minecraft:hopper".to_owned(),
            fields: BTreeMap::from([
                (
                    "Items".to_owned(),
                    serde_json::json!([{
                        "Slot": 0,
                        "id": "minecraft:diamond_sword",
                        "count": 1,
                        "components": {
                            "minecraft:custom_name": "{\"text\":\"Old\"}",
                        },
                    }]),
                ),
                (
                    "inventory".to_owned(),
                    serde_json::json!([{
                        "slot": 0,
                        "item_id": "minecraft:stone",
                        "count": 1,
                    }]),
                ),
            ]),
        };
        let replaced = network_nbt(&replaced).unwrap();
        assert!(replaced.windows("stone".len()).any(|value| value == b"stone"));
        assert!(!replaced.windows("custom_name".len()).any(|value| value == b"custom_name"));
    }
}
