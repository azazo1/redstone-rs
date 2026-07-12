use std::collections::BTreeMap;

use redstone_core::{BlockEntityData, BlockPos, EntityData, EntityId, SparseWorld};
use serde_json::{Map, Value};

use super::super::ItemStack;
use crate::rules::{Java26Rules, StateDefinition};

const ITEM_HALF_WIDTH: f64 = 0.125;
const ITEM_HEIGHT: f64 = 0.25;

pub(super) fn read_inventory(data: &BlockEntityData) -> Vec<ItemStack> {
    read_inventory_fields(&data.fields, &data.kind)
}

pub(super) fn read_inventory_fields(
    fields: &BTreeMap<String, Value>,
    kind: &str,
) -> Vec<ItemStack> {
    let mut stacks = fields
        .get("inventory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(value_to_stack)
        .collect::<Vec<_>>();
    if stacks.is_empty() {
        let raw = match kind {
            "minecraft:jukebox" => fields.get("RecordItem").or_else(|| fields.get("record_item")),
            "minecraft:decorated_pot" => fields.get("item").or_else(|| fields.get("Item")),
            _ => None,
        };
        if let Some(stack) = raw.and_then(value_to_raw_stack) {
            stacks.push(stack);
        }
    }
    if stacks.is_empty()
        && let (Some(item_id), Some(count)) = (
            fields.get("item_id").and_then(Value::as_str),
            fields.get("item_count").and_then(Value::as_i64),
        )
        && count > 0
    {
        stacks.push(ItemStack {
            slot: 0,
            item_id: item_id.to_owned(),
            count,
            components: fields.get("item_components").cloned(),
        });
    }
    stacks.sort_by_key(|stack| stack.slot);
    stacks
}

fn value_to_stack(value: &Value) -> Option<ItemStack> {
    let object = value.as_object()?;
    let slot = object.get("slot")?.as_u64()?.try_into().ok()?;
    let item_id = object.get("item_id")?.as_str()?.to_owned();
    let count = object.get("count")?.as_i64()?;
    (count > 0).then(|| ItemStack {
        slot,
        item_id,
        count,
        components: object.get("components").cloned(),
    })
}

fn value_to_raw_stack(value: &Value) -> Option<ItemStack> {
    let object = value.as_object()?;
    let item_id = object
        .get("id")
        .or_else(|| object.get("item_id"))?
        .as_str()?
        .to_owned();
    let count = object
        .get("count")
        .or_else(|| object.get("Count"))
        .and_then(Value::as_i64)
        .unwrap_or(1);
    (count > 0).then(|| ItemStack {
        slot: 0,
        item_id,
        count,
        components: object.get("components").cloned(),
    })
}

pub(super) fn write_inventory_fields(
    fields: &mut BTreeMap<String, Value>,
    kind: &str,
    stacks: &[ItemStack],
) {
    let values = stacks.iter().map(stack_to_value).collect::<Vec<_>>();
    let count = stacks.iter().map(|stack| stack.count).sum::<i64>();
    fields.insert("inventory".to_owned(), Value::Array(values));
    fields.insert("item_count".to_owned(), Value::from(count));
    if let Some(first) = stacks.first() {
        fields.insert("item_id".to_owned(), Value::String(first.item_id.clone()));
    } else {
        fields.remove("item_id");
    }
    if matches!(kind, "minecraft:jukebox" | "minecraft:decorated_pot") {
        let key = if kind == "minecraft:jukebox" {
            "RecordItem"
        } else {
            "item"
        };
        if let Some(first) = stacks.first() {
            let mut raw = Map::from_iter([
                ("id".to_owned(), Value::String(first.item_id.clone())),
                ("count".to_owned(), Value::from(first.count)),
            ]);
            if let Some(components) = &first.components {
                raw.insert("components".to_owned(), components.clone());
            }
            fields.insert(key.to_owned(), Value::Object(raw));
        } else {
            fields.remove(key);
        }
    }
}

fn stack_to_value(stack: &ItemStack) -> Value {
    let mut value = Map::from_iter([
        ("slot".to_owned(), Value::from(stack.slot)),
        ("item_id".to_owned(), Value::String(stack.item_id.clone())),
        ("count".to_owned(), Value::from(stack.count)),
    ]);
    if let Some(components) = &stack.components {
        value.insert("components".to_owned(), components.clone());
    }
    Value::Object(value)
}

pub(super) fn changed_slot(before: &[ItemStack], after: &[ItemStack]) -> Option<u32> {
    (0..6).find(|slot| {
        before.iter().find(|stack| stack.slot == *slot)
            != after.iter().find(|stack| stack.slot == *slot)
    })
}

pub(super) fn is_block_container(data: &BlockEntityData) -> bool {
    data.fields.contains_key("inventory")
        || data.fields.contains_key("capacity")
        || data.kind.ends_with("_shulker_box")
        || matches!(
            data.kind.as_str(),
            "minecraft:hopper"
                | "minecraft:dropper"
                | "minecraft:dispenser"
                | "minecraft:crafter"
                | "minecraft:chest"
                | "minecraft:trapped_chest"
                | "minecraft:barrel"
                | "minecraft:shulker_box"
                | "minecraft:furnace"
                | "minecraft:blast_furnace"
                | "minecraft:smoker"
                | "minecraft:brewing_stand"
                | "minecraft:jukebox"
                | "minecraft:chiseled_bookshelf"
                | "minecraft:decorated_pot"
        )
}

pub(super) fn is_container_entity(entity: &EntityData) -> bool {
    entity.fields.contains_key("inventory")
        || entity.fields.contains_key("capacity")
        || matches!(
            entity.kind.as_str(),
            "minecraft:hopper_minecart"
                | "minecraft:chest_minecart"
                | "minecraft:chest_boat"
                | "minecraft:chest_raft"
        )
}

pub(super) fn default_slots(kind: &str) -> Option<u32> {
    match kind {
        "minecraft:hopper" | "minecraft:hopper_minecart" | "minecraft:brewing_stand" => Some(5),
        "minecraft:dispenser" | "minecraft:dropper" | "minecraft:crafter" => Some(9),
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => Some(3),
        "minecraft:chest"
        | "minecraft:trapped_chest"
        | "minecraft:barrel"
        | "minecraft:chest_minecart"
        | "minecraft:chest_boat"
        | "minecraft:chest_raft" => Some(27),
        "minecraft:jukebox" | "minecraft:decorated_pot" => Some(1),
        "minecraft:chiseled_bookshelf" => Some(6),
        value if value.ends_with("_shulker_box") || value == "minecraft:shulker_box" => Some(27),
        _ => None,
    }
}

pub(super) fn double_chest(
    world: &SparseWorld,
    rules: &Java26Rules,
    pos: BlockPos,
    state: &StateDefinition,
) -> Option<(BlockPos, BlockPos)> {
    let chest_type = state.property("type")?;
    if chest_type == "single" {
        return None;
    }
    let facing = state.direction_property("facing")?;
    let partner_direction = if chest_type == "left" {
        facing.clockwise()
    } else {
        facing.counter_clockwise()
    };
    let partner_pos = pos.relative(partner_direction);
    let partner = rules.state(world.get_block(partner_pos)).ok()?;
    if partner.name != state.name
        || partner.direction_property("facing") != Some(facing)
        || partner.property("type") == Some(chest_type)
        || world.block_entity(partner_pos).is_none()
    {
        return None;
    }
    Some(if chest_type == "right" {
        (pos, partner_pos)
    } else {
        (partner_pos, pos)
    })
}

pub(super) fn is_jukebox_playable(item_id: &str, components: Option<&Value>) -> bool {
    item_id.starts_with("minecraft:music_disc_")
        || components.is_some_and(|components| components.get("minecraft:jukebox_playable").is_some())
}

pub(super) fn is_bookshelf_book(item_id: &str) -> bool {
    matches!(
        item_id,
        "minecraft:book"
            | "minecraft:written_book"
            | "minecraft:enchanted_book"
            | "minecraft:writable_book"
            | "minecraft:knowledge_book"
    )
}

pub(super) fn item_entities_in_suck_aabb(
    world: &SparseWorld,
    hopper: BlockPos,
    inside_block_only: bool,
) -> Vec<EntityId> {
    let min = [hopper.x as f64, hopper.y as f64 + 0.6875, hopper.z as f64];
    let max = [hopper.x as f64 + 1.0, hopper.y as f64 + 2.0, hopper.z as f64 + 1.0];
    item_entities_intersecting(world, min, max)
        .into_iter()
        .filter(|id| {
            !inside_block_only
                || world.entity(*id).is_some_and(|entity| {
                    item_aabb(entity.position).intersects(
                        [hopper.x as f64, hopper.y as f64, hopper.z as f64],
                        [hopper.x as f64 + 1.0, hopper.y as f64 + 1.0, hopper.z as f64 + 1.0],
                    )
                })
        })
        .collect()
}

pub(super) fn item_entities_for_minecart_suck(
    world: &SparseWorld,
    position: [f64; 3],
) -> Vec<EntityId> {
    item_entities_intersecting(
        world,
        [position[0] - 0.5, position[1] + 0.6875, position[2] - 0.5],
        [position[0] + 0.5, position[1] + 2.0, position[2] + 0.5],
    )
}

pub(super) fn item_entities_near_minecart(
    world: &SparseWorld,
    position: [f64; 3],
) -> Vec<EntityId> {
    item_entities_intersecting(
        world,
        [position[0] - 0.74, position[1], position[2] - 0.74],
        [position[0] + 0.74, position[1] + 0.7, position[2] + 0.74],
    )
}

fn item_entities_intersecting(
    world: &SparseWorld,
    min: [f64; 3],
    max: [f64; 3],
) -> Vec<EntityId> {
    world
        .entity_ids_in_aabb(
            [min[0] - ITEM_HALF_WIDTH, min[1] - ITEM_HEIGHT, min[2] - ITEM_HALF_WIDTH],
            [max[0] + ITEM_HALF_WIDTH, max[1], max[2] + ITEM_HALF_WIDTH],
        )
        .into_iter()
        .filter(|id| {
            world.entity(*id).is_some_and(|entity| {
                entity.kind == "minecraft:item"
                    && entity.fields.get("item_count").and_then(Value::as_i64).unwrap_or(1) > 0
                    && item_aabb(entity.position).intersects(min, max)
            })
        })
        .collect()
}

struct Aabb {
    min: [f64; 3],
    max: [f64; 3],
}

impl Aabb {
    fn intersects(&self, min: [f64; 3], max: [f64; 3]) -> bool {
        self.max[0] > min[0]
            && self.min[0] < max[0]
            && self.max[1] > min[1]
            && self.min[1] < max[1]
            && self.max[2] > min[2]
            && self.min[2] < max[2]
    }
}

fn item_aabb(position: [f64; 3]) -> Aabb {
    Aabb {
        min: [position[0] - ITEM_HALF_WIDTH, position[1], position[2] - ITEM_HALF_WIDTH],
        max: [position[0] + ITEM_HALF_WIDTH, position[1] + ITEM_HEIGHT, position[2] + ITEM_HALF_WIDTH],
    }
}

pub(super) fn block_center(pos: BlockPos) -> [f64; 3] {
    [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5]
}

pub(super) fn block_pos(position: [f64; 3]) -> BlockPos {
    BlockPos::new(
        position[0].floor() as i32,
        position[1].floor() as i32,
        position[2].floor() as i32,
    )
}
