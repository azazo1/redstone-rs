use std::collections::{BTreeMap, BTreeSet};

use redstone_core::{
    BlockPos, Direction, EntityData, EventContext, RulesError, SparseWorld, TickPriority,
};
use serde_json::{Map, Value};

use super::{BlockBehavior, Java26Rules, StateDefinition};

const DEFAULT_STACK_SIZE: i64 = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ItemStack {
    slot: u32,
    item_id: String,
    count: i64,
}

impl Java26Rules {
    pub(super) fn tick_container(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<(), RulesError> {
        let powered = self.is_powered(ctx.world, pos);
        if matches!(state.behavior, BlockBehavior::Hopper) {
            let enabled = !powered;
            if state.bool_property("enabled") != enabled {
                let next = self.changed_state(state.id, "enabled", enabled.to_string())?;
                self.set_state_and_notify(ctx, pos, next, "hopper_enabled", None)?;
            }
            if !enabled {
                return Ok(());
            }
            let cooldown = block_entity_i64(ctx.world, pos, "cooldown").unwrap_or(0);
            if cooldown > 0 {
                set_block_entity_i64(ctx.world, pos, "cooldown", cooldown - 1);
                return Ok(());
            }
            let facing = state.direction_property("facing").unwrap_or(Direction::Down);
            let target = pos.relative(facing);
            let moved = transfer_one_item(ctx.world, pos, target)
                || transfer_one_item(ctx.world, pos.relative(Direction::Up), pos)
                || absorb_item_entity(ctx.world, pos);
            if moved {
                set_block_entity_i64(ctx.world, pos, "cooldown", 8);
                *self.event_counts.entry("hopper_transfer".to_owned()).or_default() += 1;
            }
        }
        Ok(())
    }

    pub(super) fn execute_container_tick(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<(), RulesError> {
        match state.behavior {
            BlockBehavior::Dropper => self.execute_dropper(ctx, pos, state),
            BlockBehavior::Dispenser => self.execute_dispenser(ctx, pos, state),
            BlockBehavior::Crafter => self.execute_crafter(ctx, pos, state)?,
            _ => {}
        }
        Ok(())
    }

    fn execute_dropper(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) {
        let Some(index) = random_stack_index(ctx, pos) else {
            return;
        };
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let target = pos.relative(facing);
        let item = inventory(ctx.world, pos)[index].item_id.clone();
        if insert_item(ctx.world, target, &item, 1) == 1 {
            take_item_at(ctx.world, pos, index, 1);
            *self.event_counts.entry("dropper_transfer".to_owned()).or_default() += 1;
        } else if take_item_at(ctx.world, pos, index, 1).is_some() {
            spawn_item(ctx.world, target, item, 1);
            *self.event_counts.entry("dropper_eject".to_owned()).or_default() += 1;
        }
    }

    fn execute_dispenser(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) {
        let Some(index) = random_stack_index(ctx, pos) else {
            return;
        };
        let item = inventory(ctx.world, pos)[index].item_id.clone();
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let target = pos.relative(facing);
        match dispenser_behavior(&item) {
            Some(DispenserBehavior::Projectile) => {
                if take_item_at(ctx.world, pos, index, 1).is_some() {
                    ctx.world.spawn_entity(EntityData {
                        kind: item,
                        position: block_center(target),
                        fields: BTreeMap::new(),
                    });
                    *self.event_counts.entry("dispenser_projectile".to_owned()).or_default() += 1;
                }
            }
            Some(DispenserBehavior::PrimeTnt) => {
                if take_item_at(ctx.world, pos, index, 1).is_some() {
                    ctx.unsupported(target, "dispenser_tnt_explosion");
                    *self.event_counts.entry("dispenser_tnt".to_owned()).or_default() += 1;
                }
            }
            Some(DispenserBehavior::DefaultEject) => {
                if take_item_at(ctx.world, pos, index, 1).is_some() {
                    spawn_item(ctx.world, target, item, 1);
                    *self.event_counts.entry("dispenser_eject".to_owned()).or_default() += 1;
                }
            }
            None => ctx.unsupported(pos, format!("dispenser_item:{item}")),
        }
    }

    fn execute_crafter(
        &mut self,
        ctx: &mut EventContext<'_>,
        pos: BlockPos,
        state: &StateDefinition,
    ) -> Result<(), RulesError> {
        if state.bool_property("crafting") {
            let idle = self.changed_state(state.id, "crafting", "false")?;
            self.set_state_and_notify(ctx, pos, idle, "crafter_idle", None)?;
            return Ok(());
        }
        let Some(item) = take_first_item(ctx.world, pos, 1) else {
            return Ok(());
        };
        let output_item = block_entity_string(ctx.world, pos, "output_item_id")
            .unwrap_or_else(|| item.item_id.clone());
        let output_count = block_entity_i64(ctx.world, pos, "output_count").unwrap_or(1).max(1);
        let front = crafter_front(state);
        let target = pos.relative(front);
        let inserted = insert_item(ctx.world, target, &output_item, output_count);
        if inserted < output_count {
            spawn_item(ctx.world, target, output_item, output_count - inserted);
        }
        let crafting = self.changed_state(state.id, "crafting", "true")?;
        self.set_state_and_notify(ctx, pos, crafting, "crafter_craft", None)?;
        ctx.schedule_tick(pos, state.kind, 1, TickPriority::Normal);
        *self.event_counts.entry("crafter_craft".to_owned()).or_default() += 1;
        Ok(())
    }
}

pub(super) fn block_entity_i64(world: &SparseWorld, pos: BlockPos, key: &str) -> Option<i64> {
    world.block_entity(pos)?.fields.get(key)?.as_i64()
}

fn block_entity_string(world: &SparseWorld, pos: BlockPos, key: &str) -> Option<String> {
    world
        .block_entity(pos)?
        .fields
        .get(key)?
        .as_str()
        .map(ToOwned::to_owned)
}

fn set_block_entity_i64(world: &mut SparseWorld, pos: BlockPos, key: &str, value: i64) {
    if let Some(data) = world.block_entity_mut(pos) {
        data.fields.insert(key.to_owned(), Value::from(value));
    }
}

pub(super) fn container_signal(world: &SparseWorld, pos: BlockPos) -> Option<u8> {
    if !is_container(world, pos) {
        return None;
    }
    let data = world.block_entity(pos)?;
    let stacks = inventory(world, pos);
    let capacity = data
        .fields
        .get("capacity")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_STACK_SIZE)
        .max(1);
    let count = stacks.iter().map(|stack| stack.count).sum::<i64>();
    Some(if count == 0 {
        0
    } else {
        (1 + count * 14 / capacity).clamp(1, 15) as u8
    })
}

fn transfer_one_item(world: &mut SparseWorld, source: BlockPos, target: BlockPos) -> bool {
    let Some(stack) = inventory(world, source).first().cloned() else {
        return false;
    };
    if insert_item(world, target, &stack.item_id, 1) != 1 {
        return false;
    }
    take_item_from_slot(world, source, stack.slot, 1).is_some()
}

fn insert_item(world: &mut SparseWorld, target: BlockPos, item_id: &str, count: i64) -> i64 {
    if count <= 0 || !is_container(world, target) {
        return 0;
    }
    let mut stacks = inventory(world, target);
    let capacity = block_entity_i64(world, target, "capacity")
        .unwrap_or(DEFAULT_STACK_SIZE)
        .max(1);
    let current = stacks.iter().map(|stack| stack.count).sum::<i64>();
    let mut remaining = count.min(capacity.saturating_sub(current));
    let inserted = remaining;
    for stack in stacks.iter_mut().filter(|stack| stack.item_id == item_id) {
        let moved = remaining.min(DEFAULT_STACK_SIZE.saturating_sub(stack.count));
        stack.count += moved;
        remaining -= moved;
        if remaining == 0 {
            break;
        }
    }
    let slot_count = container_slot_count(world, target, capacity);
    let mut used = stacks.iter().map(|stack| stack.slot).collect::<BTreeSet<_>>();
    while remaining > 0 {
        let Some(slot) = (0..slot_count).find(|slot| !used.contains(slot)) else {
            break;
        };
        let moved = remaining.min(DEFAULT_STACK_SIZE);
        stacks.push(ItemStack {
            slot,
            item_id: item_id.to_owned(),
            count: moved,
        });
        used.insert(slot);
        remaining -= moved;
    }
    write_inventory(world, target, stacks);
    inserted - remaining
}

fn random_stack_index(ctx: &mut EventContext<'_>, pos: BlockPos) -> Option<usize> {
    let len = inventory(ctx.world, pos).len();
    (len > 0).then(|| ctx.random_bounded(len as u32) as usize)
}

fn take_first_item(world: &mut SparseWorld, pos: BlockPos, count: i64) -> Option<ItemStack> {
    let stack = inventory(world, pos).first().cloned()?;
    take_item_from_slot(world, pos, stack.slot, count)
}

fn take_item_at(
    world: &mut SparseWorld,
    pos: BlockPos,
    index: usize,
    count: i64,
) -> Option<ItemStack> {
    let stack = inventory(world, pos).get(index).cloned()?;
    take_item_from_slot(world, pos, stack.slot, count)
}

fn take_item_from_slot(
    world: &mut SparseWorld,
    pos: BlockPos,
    slot: u32,
    count: i64,
) -> Option<ItemStack> {
    let mut stacks = inventory(world, pos);
    let index = stacks.iter().position(|stack| stack.slot == slot)?;
    let taken = count.max(0).min(stacks[index].count);
    if taken == 0 {
        return None;
    }
    let result = ItemStack {
        slot,
        item_id: stacks[index].item_id.clone(),
        count: taken,
    };
    stacks[index].count -= taken;
    write_inventory(world, pos, stacks);
    Some(result)
}

fn inventory(world: &SparseWorld, pos: BlockPos) -> Vec<ItemStack> {
    let Some(data) = world.block_entity(pos) else {
        return Vec::new();
    };
    let mut stacks = data
        .fields
        .get("inventory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            let slot = entry.get("slot")?.as_u64()?.try_into().ok()?;
            let item_id = entry.get("item_id")?.as_str()?.to_owned();
            let count = entry.get("count")?.as_i64()?;
            (count > 0).then_some(ItemStack {
                slot,
                item_id,
                count,
            })
        })
        .collect::<Vec<_>>();
    if stacks.is_empty()
        && let (Some(item_id), Some(count)) = (
            data.fields.get("item_id").and_then(Value::as_str),
            data.fields.get("item_count").and_then(Value::as_i64),
        )
        && count > 0
    {
        stacks.push(ItemStack {
            slot: 0,
            item_id: item_id.to_owned(),
            count,
        });
    }
    stacks.sort_by_key(|stack| stack.slot);
    stacks
}

fn write_inventory(world: &mut SparseWorld, pos: BlockPos, mut stacks: Vec<ItemStack>) {
    stacks.retain(|stack| stack.count > 0);
    stacks.sort_by_key(|stack| stack.slot);
    let count = stacks.iter().map(|stack| stack.count).sum::<i64>();
    let inventory = stacks
        .iter()
        .map(|stack| {
            Value::Object(Map::from_iter([
                ("slot".to_owned(), Value::from(stack.slot)),
                ("item_id".to_owned(), Value::String(stack.item_id.clone())),
                ("count".to_owned(), Value::from(stack.count)),
            ]))
        })
        .collect::<Vec<_>>();
    if let Some(data) = world.block_entity_mut(pos) {
        data.fields
            .insert("inventory".to_owned(), Value::Array(inventory));
        data.fields.insert("item_count".to_owned(), Value::from(count));
        if let Some(first) = stacks.first() {
            data.fields
                .insert("item_id".to_owned(), Value::String(first.item_id.clone()));
        } else {
            data.fields.remove("item_id");
        }
    }
}

fn container_slot_count(world: &SparseWorld, pos: BlockPos, capacity: i64) -> u32 {
    block_entity_i64(world, pos, "slot_count")
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            ((capacity + DEFAULT_STACK_SIZE - 1) / DEFAULT_STACK_SIZE).max(1) as u32
        })
}

fn is_container(world: &SparseWorld, pos: BlockPos) -> bool {
    let Some(data) = world.block_entity(pos) else {
        return false;
    };
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
        )
}

fn spawn_item(world: &mut SparseWorld, pos: BlockPos, item_id: String, count: i64) {
    if count <= 0 {
        return;
    }
    let fields = BTreeMap::from([
        ("item_id".to_owned(), Value::String(item_id)),
        ("item_count".to_owned(), Value::from(count)),
        ("age".to_owned(), Value::from(0)),
        ("pickup_delay".to_owned(), Value::from(10)),
    ]);
    world.spawn_entity(EntityData {
        kind: "minecraft:item".to_owned(),
        position: block_center(pos),
        fields,
    });
}

fn absorb_item_entity(world: &mut SparseWorld, hopper: BlockPos) -> bool {
    let candidate = world
        .entity_ids_in_aabb(
            [hopper.x as f64 - 0.25, hopper.y as f64, hopper.z as f64 - 0.25],
            [
                hopper.x as f64 + 1.25,
                hopper.y as f64 + 1.5,
                hopper.z as f64 + 1.25,
            ],
        )
        .into_iter()
        .find_map(|id| {
            let entity = world.entity(id)?;
            (entity.kind == "minecraft:item").then(|| {
            (
                id,
                entity.fields.get("item_id").and_then(Value::as_str).map(ToOwned::to_owned),
                entity.fields.get("item_count").and_then(Value::as_i64).unwrap_or(1),
            )
            })
        });
    let Some((entity_id, Some(item_id), entity_count)) = candidate else {
        return false;
    };
    if insert_item(world, hopper, &item_id, 1) != 1 {
        return false;
    }
    if entity_count <= 1 {
        world.remove_entity(entity_id);
    } else if let Some(fields) = world.entity_fields_mut(entity_id) {
        fields.insert("item_count".to_owned(), Value::from(entity_count - 1));
    }
    true
}

fn block_center(pos: BlockPos) -> [f64; 3] {
    [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5]
}

fn crafter_front(state: &StateDefinition) -> Direction {
    state
        .property("orientation")
        .and_then(|value| value.split_once('_').map(|(front, _)| front))
        .and_then(|front| match front {
            "west" => Some(Direction::West),
            "east" => Some(Direction::East),
            "down" => Some(Direction::Down),
            "up" => Some(Direction::Up),
            "north" => Some(Direction::North),
            "south" => Some(Direction::South),
            _ => None,
        })
        .unwrap_or(Direction::North)
}

enum DispenserBehavior {
    Projectile,
    PrimeTnt,
    DefaultEject,
}

fn dispenser_behavior(item: &str) -> Option<DispenserBehavior> {
    match item {
        "minecraft:arrow"
        | "minecraft:tipped_arrow"
        | "minecraft:spectral_arrow"
        | "minecraft:egg"
        | "minecraft:blue_egg"
        | "minecraft:brown_egg"
        | "minecraft:snowball"
        | "minecraft:experience_bottle"
        | "minecraft:splash_potion"
        | "minecraft:lingering_potion"
        | "minecraft:firework_rocket"
        | "minecraft:fire_charge"
        | "minecraft:wind_charge" => Some(DispenserBehavior::Projectile),
        "minecraft:tnt" => Some(DispenserBehavior::PrimeTnt),
        item if item.ends_with("_boat")
            || item.ends_with("_raft")
            || item.ends_with("_minecart") => Some(DispenserBehavior::DefaultEject),
        _ => None,
    }
}
