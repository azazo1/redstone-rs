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
    components: Option<Value>,
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
                set_block_entity_i64(ctx, pos, "cooldown", cooldown - 1);
                return Ok(());
            }
            let facing = state.direction_property("facing").unwrap_or(Direction::Down);
            let target = pos.relative(facing);
            let moved = transfer_one_item(ctx, pos, target, None, Some(facing.opposite()))
                || transfer_one_item(
                    ctx,
                    pos.relative(Direction::Up),
                    pos,
                    Some(Direction::Down),
                    None,
                )
                || absorb_item_entity(ctx, pos);
            if moved {
                set_block_entity_i64(ctx, pos, "cooldown", 8);
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
            BlockBehavior::Dropper => self.execute_dropper(ctx, pos, state)?,
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
    ) -> Result<(), RulesError> {
        let Some(index) = random_stack_index(ctx, pos) else {
            return Ok(());
        };
        let facing = state.direction_property("facing").unwrap_or(Direction::North);
        let target = pos.relative(facing);
        let item = inventory(ctx.world, pos)[index].item_id.clone();
        if insert_item(ctx, target, &item, 1, Some(facing.opposite())) == 1 {
            take_item_at(ctx, pos, index, 1);
            *self.event_counts.entry("dropper_transfer".to_owned()).or_default() += 1;
            self.refresh_comparators_near(ctx, target)?;
            self.refresh_comparators_near(ctx, pos)?;
        } else if !is_container(ctx.world, target)
            && take_item_at(ctx, pos, index, 1).is_some()
        {
            spawn_item(ctx.world, target, item, 1);
            *self.event_counts.entry("dropper_eject".to_owned()).or_default() += 1;
            self.refresh_comparators_near(ctx, pos)?;
        }
        Ok(())
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
                if take_item_at(ctx, pos, index, 1).is_some() {
                    let velocity = facing.step();
                    ctx.world.spawn_entity(EntityData {
                        kind: projectile_entity_kind(&item).to_owned(),
                        position: block_center(target),
                        fields: BTreeMap::from([
                            ("source_item".to_owned(), Value::String(item)),
                            (
                                "facing".to_owned(),
                                Value::String(direction_name(facing).to_owned()),
                            ),
                            (
                                "velocity".to_owned(),
                                serde_json::json!([velocity.0, velocity.1, velocity.2]),
                            ),
                        ]),
                    });
                    *self.event_counts.entry("dispenser_projectile".to_owned()).or_default() += 1;
                }
            }
            Some(DispenserBehavior::PrimeTnt) => {
                if take_item_at(ctx, pos, index, 1).is_some() {
                    ctx.world.spawn_entity(EntityData {
                        kind: "minecraft:tnt".to_owned(),
                        position: [target.x as f64 + 0.5, target.y as f64, target.z as f64 + 0.5],
                        fields: BTreeMap::from([
                            ("fuse".to_owned(), Value::from(80)),
                            ("explosion_power".to_owned(), Value::from(4)),
                            ("ignited_by".to_owned(), Value::String("dispenser".to_owned())),
                        ]),
                    });
                    *self.event_counts.entry("dispenser_tnt".to_owned()).or_default() += 1;
                }
            }
            Some(DispenserBehavior::Minecart) => {
                if let Some(position) = minecart_spawn_position(self, ctx.world, pos, facing) {
                    if take_item_at(ctx, pos, index, 1).is_some() {
                    ctx.world.spawn_entity(EntityData {
                            kind: item.clone(),
                        position,
                        fields: BTreeMap::new(),
                    });
                        *self
                            .event_counts
                            .entry("dispenser_minecart".to_owned())
                            .or_default() += 1;
                    }
                } else if take_item_at(ctx, pos, index, 1).is_some() {
                    spawn_item(ctx.world, target, item, 1);
                    *self.event_counts.entry("dispenser_eject".to_owned()).or_default() += 1;
                }
            }
            Some(DispenserBehavior::DefaultEject) => {
                if take_item_at(ctx, pos, index, 1).is_some() {
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
        let Some(item) = take_first_enabled_item(ctx, pos, 1) else {
            return Ok(());
        };
        let output_item = block_entity_string(ctx.world, pos, "output_item_id")
            .unwrap_or_else(|| item.item_id.clone());
        let output_count = block_entity_i64(ctx.world, pos, "output_count").unwrap_or(1).max(1);
        let front = crafter_front(state);
        let target = pos.relative(front);
        let inserted = insert_item(
            ctx,
            target,
            &output_item,
            output_count,
            Some(front.opposite()),
        );
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

fn set_block_entity_i64(ctx: &mut EventContext<'_>, pos: BlockPos, key: &str, value: i64) {
    ctx.update_block_entity(pos, |data| {
        data.fields.insert(key.to_owned(), Value::from(value));
    });
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

fn transfer_one_item(
    ctx: &mut EventContext<'_>,
    source: BlockPos,
    target: BlockPos,
    source_face: Option<Direction>,
    target_face: Option<Direction>,
) -> bool {
    let stacks = inventory(ctx.world, source);
    for slot in slots_for_face(ctx.world, source, source_face) {
        let Some(stack) = stacks.iter().find(|stack| stack.slot == slot) else {
            continue;
        };
        if !can_take_from_slot(ctx.world, source, stack, source_face) {
            continue;
        }
        if insert_item_with_components(
            ctx,
            target,
            &stack.item_id,
            1,
            target_face,
            stack.components.as_ref(),
        ) != 1
        {
            continue;
        }
        return take_item_from_slot(ctx, source, stack.slot, 1).is_some();
    }
    false
}

fn insert_item(
    ctx: &mut EventContext<'_>,
    target: BlockPos,
    item_id: &str,
    count: i64,
    face: Option<Direction>,
) -> i64 {
    insert_item_with_components(ctx, target, item_id, count, face, None)
}

fn insert_item_with_components(
    ctx: &mut EventContext<'_>,
    target: BlockPos,
    item_id: &str,
    count: i64,
    face: Option<Direction>,
    components: Option<&Value>,
) -> i64 {
    if count <= 0 || !is_container(ctx.world, target) {
        return 0;
    }
    let mut stacks = inventory(ctx.world, target);
    let slots = slots_for_face(ctx.world, target, face);
    let capacity = block_entity_i64(ctx.world, target, "capacity")
        .unwrap_or(DEFAULT_STACK_SIZE)
        .max(1);
    let current = stacks.iter().map(|stack| stack.count).sum::<i64>();
    let mut remaining = count.min(capacity.saturating_sub(current));
    let inserted = remaining;
    for stack in stacks.iter_mut().filter(|stack| {
        stack.item_id == item_id
            && stack.components.as_ref() == components
            && slots.contains(&stack.slot)
            && can_place_in_slot(ctx.world, target, stack.slot, item_id)
    }) {
        let available = slot_stack_limit(ctx.world, target, stack.slot, item_id)
            .saturating_sub(stack.count)
            .max(0);
        let moved = remaining.min(available);
        stack.count += moved;
        remaining -= moved;
        if remaining == 0 {
            break;
        }
    }
    let mut used = stacks.iter().map(|stack| stack.slot).collect::<BTreeSet<_>>();
    while remaining > 0 {
        let Some(slot) = slots
            .iter()
            .copied()
            .find(|slot| {
                !used.contains(slot) && can_place_in_slot(ctx.world, target, *slot, item_id)
            })
        else {
            break;
        };
        let moved = remaining.min(slot_stack_limit(ctx.world, target, slot, item_id));
        stacks.push(ItemStack {
            slot,
            item_id: item_id.to_owned(),
            count: moved,
            components: components.cloned(),
        });
        used.insert(slot);
        remaining -= moved;
    }
    write_inventory(ctx, target, stacks);
    inserted - remaining
}

fn random_stack_index(ctx: &mut EventContext<'_>, pos: BlockPos) -> Option<usize> {
    let len = inventory(ctx.world, pos).len();
    (len > 0).then(|| ctx.random_bounded(len as u32) as usize)
}

fn take_first_enabled_item(
    ctx: &mut EventContext<'_>,
    pos: BlockPos,
    count: i64,
) -> Option<ItemStack> {
    let disabled = disabled_slots(ctx.world, pos);
    let stack = inventory(ctx.world, pos)
        .into_iter()
        .find(|stack| !disabled.contains(&stack.slot))?;
    take_item_from_slot(ctx, pos, stack.slot, count)
}

fn take_item_at(
    ctx: &mut EventContext<'_>,
    pos: BlockPos,
    index: usize,
    count: i64,
) -> Option<ItemStack> {
    let stack = inventory(ctx.world, pos).get(index).cloned()?;
    take_item_from_slot(ctx, pos, stack.slot, count)
}

fn take_item_from_slot(
    ctx: &mut EventContext<'_>,
    pos: BlockPos,
    slot: u32,
    count: i64,
) -> Option<ItemStack> {
    let mut stacks = inventory(ctx.world, pos);
    let index = stacks.iter().position(|stack| stack.slot == slot)?;
    let taken = count.max(0).min(stacks[index].count);
    if taken == 0 {
        return None;
    }
    let result = ItemStack {
        slot,
        item_id: stacks[index].item_id.clone(),
        count: taken,
        components: stacks[index].components.clone(),
    };
    stacks[index].count -= taken;
    write_inventory(ctx, pos, stacks);
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
            let components = entry.get("components").cloned();
            (count > 0).then_some(ItemStack {
                slot,
                item_id,
                count,
                components,
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
            components: None,
        });
    }
    stacks.sort_by_key(|stack| stack.slot);
    stacks
}

fn write_inventory(ctx: &mut EventContext<'_>, pos: BlockPos, mut stacks: Vec<ItemStack>) {
    stacks.retain(|stack| stack.count > 0);
    stacks.sort_by_key(|stack| stack.slot);
    let count = stacks.iter().map(|stack| stack.count).sum::<i64>();
    let inventory = stacks
        .iter()
        .map(|stack| {
            let mut item = Map::from_iter([
                ("slot".to_owned(), Value::from(stack.slot)),
                ("item_id".to_owned(), Value::String(stack.item_id.clone())),
                ("count".to_owned(), Value::from(stack.count)),
            ]);
            if let Some(components) = &stack.components {
                item.insert("components".to_owned(), components.clone());
            }
            Value::Object(item)
        })
        .collect::<Vec<_>>();
    ctx.update_block_entity(pos, |data| {
        data.fields
            .insert("inventory".to_owned(), Value::Array(inventory));
        data.fields.insert("item_count".to_owned(), Value::from(count));
        if let Some(first) = stacks.first() {
            data.fields
                .insert("item_id".to_owned(), Value::String(first.item_id.clone()));
        } else {
            data.fields.remove("item_id");
        }
    });
}

fn container_slot_count(world: &SparseWorld, pos: BlockPos, capacity: i64) -> u32 {
    block_entity_i64(world, pos, "slot_count")
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| {
            ((capacity + DEFAULT_STACK_SIZE - 1) / DEFAULT_STACK_SIZE).max(1) as u32
        })
}

fn slots_for_face(
    world: &SparseWorld,
    pos: BlockPos,
    face: Option<Direction>,
) -> Vec<u32> {
    let capacity = block_entity_i64(world, pos, "capacity")
        .unwrap_or(DEFAULT_STACK_SIZE)
        .max(1);
    let all = || (0..container_slot_count(world, pos, capacity)).collect::<Vec<_>>();
    let Some(face) = face else {
        return all();
    };
    match container_kind(world, pos) {
        Some("minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker") => {
            match face {
                Direction::Up => vec![0],
                Direction::Down => vec![2, 1],
                _ => vec![1],
            }
        }
        Some("minecraft:brewing_stand") => match face {
            Direction::Up => vec![3],
            Direction::Down => vec![0, 1, 2, 3],
            _ => vec![0, 1, 2, 4],
        },
        _ => all(),
    }
}

fn can_place_in_slot(world: &SparseWorld, pos: BlockPos, slot: u32, item_id: &str) -> bool {
    let stacks = inventory(world, pos);
    let current = stacks.iter().find(|stack| stack.slot == slot);
    match container_kind(world, pos) {
        Some("minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker") => {
            match slot {
                0 => true,
                1 => {
                    is_furnace_fuel(item_id)
                        || item_id == "minecraft:bucket"
                            && current.is_none_or(|stack| stack.item_id != "minecraft:bucket")
                }
                _ => false,
            }
        }
        Some("minecraft:brewing_stand") => match slot {
            0..=2 => is_brewing_bottle(item_id) && current.is_none(),
            3 => is_brewing_ingredient(item_id),
            4 => item_id == "minecraft:blaze_powder",
            _ => false,
        },
        Some("minecraft:crafter") => {
            if disabled_slots(world, pos).contains(&slot) {
                return false;
            }
            let Some(current) = current else {
                return true;
            };
            if current.item_id != item_id || current.count >= DEFAULT_STACK_SIZE {
                return false;
            }
            !(slot + 1..9).any(|later_slot| {
                if disabled_slots(world, pos).contains(&later_slot) {
                    return false;
                }
                stacks
                    .iter()
                    .find(|stack| stack.slot == later_slot)
                    .is_none_or(|later| {
                        later.item_id == item_id && later.count < current.count
                    })
            })
        }
        Some(kind) if is_shulker_box_container(kind) => !is_shulker_box_item(item_id),
        _ => true,
    }
}

fn can_take_from_slot(
    world: &SparseWorld,
    pos: BlockPos,
    stack: &ItemStack,
    face: Option<Direction>,
) -> bool {
    match (container_kind(world, pos), face) {
        (
            Some("minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker"),
            Some(Direction::Down),
        ) if stack.slot == 1 => {
            matches!(stack.item_id.as_str(), "minecraft:bucket" | "minecraft:water_bucket")
        }
        (Some("minecraft:brewing_stand"), _) if stack.slot == 3 => {
            stack.item_id == "minecraft:glass_bottle"
        }
        _ => true,
    }
}

fn slot_stack_limit(world: &SparseWorld, pos: BlockPos, slot: u32, item_id: &str) -> i64 {
    if matches!(container_kind(world, pos), Some("minecraft:brewing_stand")) && slot <= 2
        || is_unstackable_item(item_id)
    {
        1
    } else {
        DEFAULT_STACK_SIZE
    }
}

fn container_kind(world: &SparseWorld, pos: BlockPos) -> Option<&str> {
    world.block_entity(pos).map(|data| data.kind.as_str())
}

fn disabled_slots(world: &SparseWorld, pos: BlockPos) -> BTreeSet<u32> {
    world
        .block_entity(pos)
        .and_then(|data| data.fields.get("disabled_slots"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter_map(|slot| u32::try_from(slot).ok())
        .filter(|slot| *slot < 9)
        .collect()
}

fn is_furnace_fuel(item_id: &str) -> bool {
    matches!(
        item_id,
        "minecraft:lava_bucket"
            | "minecraft:coal_block"
            | "minecraft:blaze_rod"
            | "minecraft:coal"
            | "minecraft:charcoal"
            | "minecraft:note_block"
            | "minecraft:bookshelf"
            | "minecraft:chiseled_bookshelf"
            | "minecraft:lectern"
            | "minecraft:jukebox"
            | "minecraft:chest"
            | "minecraft:trapped_chest"
            | "minecraft:crafting_table"
            | "minecraft:daylight_detector"
            | "minecraft:bow"
            | "minecraft:fishing_rod"
            | "minecraft:ladder"
            | "minecraft:wooden_shovel"
            | "minecraft:wooden_sword"
            | "minecraft:wooden_spear"
            | "minecraft:wooden_hoe"
            | "minecraft:wooden_axe"
            | "minecraft:wooden_pickaxe"
            | "minecraft:stick"
            | "minecraft:bowl"
            | "minecraft:crossbow"
            | "minecraft:bamboo"
            | "minecraft:dead_bush"
            | "minecraft:short_dry_grass"
            | "minecraft:tall_dry_grass"
            | "minecraft:scaffolding"
            | "minecraft:loom"
            | "minecraft:barrel"
            | "minecraft:cartography_table"
            | "minecraft:fletching_table"
            | "minecraft:smithing_table"
            | "minecraft:composter"
            | "minecraft:azalea"
            | "minecraft:flowering_azalea"
            | "minecraft:mangrove_roots"
            | "minecraft:leaf_litter"
            | "minecraft:dried_kelp_block"
            | "minecraft:bamboo_mosaic"
    ) || is_overworld_wood_item(item_id)
        || item_id.ends_with("_wool")
        || item_id.ends_with("_carpet")
        || item_id.ends_with("_sapling")
        || item_id.ends_with("_banner")
}

fn is_overworld_wood_item(item_id: &str) -> bool {
    const WOOD_TYPES: [&str; 10] = [
        "oak",
        "spruce",
        "birch",
        "jungle",
        "acacia",
        "dark_oak",
        "pale_oak",
        "mangrove",
        "cherry",
        "bamboo",
    ];
    let path = item_id.strip_prefix("minecraft:").unwrap_or(item_id);
    WOOD_TYPES.iter().any(|wood| {
        path == format!("{wood}_planks")
            || path == format!("{wood}_mosaic")
            || path.starts_with(&format!("{wood}_"))
                && matches!(
                    path.strip_prefix(&format!("{wood}_")),
                    Some(
                        "stairs"
                            | "slab"
                            | "trapdoor"
                            | "pressure_plate"
                            | "shelf"
                            | "fence"
                            | "fence_gate"
                            | "sign"
                            | "hanging_sign"
                            | "door"
                            | "boat"
                            | "chest_boat"
                            | "raft"
                            | "chest_raft"
                            | "button"
                    )
                )
    }) || matches!(
        path,
        "bamboo_block"
            | "stripped_bamboo_block"
            | "bamboo_mosaic"
            | "bamboo_mosaic_stairs"
            | "bamboo_mosaic_slab"
    ) || is_overworld_log(path)
}

fn is_overworld_log(path: &str) -> bool {
    const LOG_TYPES: [&str; 9] = [
        "oak",
        "spruce",
        "birch",
        "jungle",
        "acacia",
        "dark_oak",
        "pale_oak",
        "mangrove",
        "cherry",
    ];
    LOG_TYPES.iter().any(|wood| {
        matches!(
            path,
            value if value == format!("{wood}_log")
                || value == format!("stripped_{wood}_log")
                || value == format!("{wood}_wood")
                || value == format!("stripped_{wood}_wood")
        )
    })
}

fn is_brewing_bottle(item_id: &str) -> bool {
    matches!(
        item_id,
        "minecraft:potion"
            | "minecraft:splash_potion"
            | "minecraft:lingering_potion"
            | "minecraft:glass_bottle"
    )
}

fn is_brewing_ingredient(item_id: &str) -> bool {
    matches!(
        item_id,
        "minecraft:nether_wart"
            | "minecraft:redstone"
            | "minecraft:glowstone_dust"
            | "minecraft:fermented_spider_eye"
            | "minecraft:gunpowder"
            | "minecraft:dragon_breath"
            | "minecraft:sugar"
            | "minecraft:rabbit_foot"
            | "minecraft:glistering_melon_slice"
            | "minecraft:spider_eye"
            | "minecraft:pufferfish"
            | "minecraft:magma_cream"
            | "minecraft:golden_carrot"
            | "minecraft:blaze_powder"
            | "minecraft:ghast_tear"
            | "minecraft:turtle_helmet"
            | "minecraft:phantom_membrane"
            | "minecraft:stone"
            | "minecraft:slime_block"
            | "minecraft:cobweb"
            | "minecraft:breeze_rod"
    )
}

fn is_shulker_box_container(kind: &str) -> bool {
    kind == "minecraft:shulker_box" || kind.ends_with("_shulker_box")
}

fn is_shulker_box_item(item_id: &str) -> bool {
    item_id == "minecraft:shulker_box" || item_id.ends_with("_shulker_box")
}

fn is_unstackable_item(item_id: &str) -> bool {
    is_shulker_box_item(item_id)
        || is_brewing_bottle(item_id) && item_id != "minecraft:glass_bottle"
        || matches!(item_id, "minecraft:water_bucket" | "minecraft:lava_bucket")
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

fn absorb_item_entity(ctx: &mut EventContext<'_>, hopper: BlockPos) -> bool {
    let candidate = ctx
        .world
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
            let entity = ctx.world.entity(id)?;
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
    if insert_item(ctx, hopper, &item_id, 1, None) != 1 {
        return false;
    }
    if entity_count <= 1 {
        ctx.world.remove_entity(entity_id);
    } else if let Some(fields) = ctx.world.entity_fields_mut(entity_id) {
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
    Minecart,
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
        "minecraft:minecart"
        | "minecraft:chest_minecart"
        | "minecraft:furnace_minecart"
        | "minecraft:tnt_minecart"
        | "minecraft:hopper_minecart"
        | "minecraft:command_block_minecart" => Some(DispenserBehavior::Minecart),
        item if item.ends_with("_boat") || item.ends_with("_raft") => {
            Some(DispenserBehavior::DefaultEject)
        }
        _ => None,
    }
}

fn projectile_entity_kind(item: &str) -> &str {
    match item {
        "minecraft:tipped_arrow" | "minecraft:spectral_arrow" => "minecraft:arrow",
        "minecraft:experience_bottle" => "minecraft:experience_bottle",
        "minecraft:splash_potion" | "minecraft:lingering_potion" => "minecraft:potion",
        other => other,
    }
}

fn minecart_spawn_position(
    rules: &Java26Rules,
    world: &SparseWorld,
    source: BlockPos,
    facing: Direction,
) -> Option<[f64; 3]> {
    let front = source.relative(facing);
    let front_state = world.get_block(front);
    let (rail_pos, offset) = if is_rail_state(rules, front_state) {
        (front, if rail_is_slope(rules, front_state) { 0.6 } else { 0.1 })
    } else if front_state == world.air()
        && is_rail_state(rules, world.get_block(front.relative(Direction::Down)))
    {
        let below = front.relative(Direction::Down);
        (
            below,
            if facing != Direction::Down && rail_is_slope(rules, world.get_block(below)) {
                0.6
            } else {
                0.1
            },
        )
    } else {
        return None;
    };
    Some([
        rail_pos.x as f64 + 0.5,
        rail_pos.y as f64 + offset,
        rail_pos.z as f64 + 0.5,
    ])
}

fn is_rail_state(rules: &Java26Rules, state: redstone_core::BlockStateId) -> bool {
    rules.state(state).is_ok_and(|state| {
        let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
        path == "rail" || path.ends_with("_rail")
    })
}

fn rail_is_slope(rules: &Java26Rules, state: redstone_core::BlockStateId) -> bool {
    rules.state(state).is_ok_and(|state| {
        matches!(
            state.property("shape"),
            Some(
                "ascending_east"
                    | "ascending_west"
                    | "ascending_north"
                    | "ascending_south"
            )
        )
    })
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
    }
}
