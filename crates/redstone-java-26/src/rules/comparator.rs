use std::collections::{BTreeMap, BTreeSet};

use redstone_core::{BlockPos, Direction, EntityData, SparseWorld};
use serde_json::Value;

use crate::Java26Registry;

use super::inventory::{
    container_signal, container_signal_for_positions, container_signal_from_fields,
};

mod block_entity;

pub(super) fn analog_output(
    registry: &Java26Registry,
    world: &SparseWorld,
    pos: BlockPos,
    direction: Direction,
) -> Option<u8> {
    if let Some(output) = block_entity_i64(world, pos, &["comparator_output"]) {
        return Some(clamp_signal(output));
    }

    let state = registry.state(world.get_block(pos))?;
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let output = match path {
        "water_cauldron" | "powder_snow_cauldron" | "composter" => {
            state.int_property("level").unwrap_or(0)
        }
        "lava_cauldron" => 3,
        "cauldron" => 0,
        "cake" => (7 - state.int_property("bites").unwrap_or(0)) * 2,
        value if value == "candle_cake" || value.ends_with("_candle_cake") => 14,
        "bee_nest" | "beehive" => state.int_property("honey_level").unwrap_or(0),
        "end_portal_frame" => i32::from(state.bool_property("eye")) * 15,
        "respawn_anchor" => state.int_property("charges").unwrap_or(0) * 15 / 4,
        value if value.ends_with("_bulb") => i32::from(state.bool_property("lit")) * 15,
        value if value.ends_with("copper_golem_statue") => {
            copper_golem_pose_output(state.property("copper_golem_pose").unwrap_or("standing"))
        }
        "chiseled_bookshelf" => {
            block_entity_i64(world, pos, &["last_interacted_slot", "LastInteractedSlot"])
                .unwrap_or(-1)
                .clamp(-1, 14) as i32
                + 1
        }
        "command_block" | "chain_command_block" | "repeating_command_block" => {
            block_entity_i64(world, pos, &["SuccessCount", "success_count"])
                .unwrap_or(0)
                .clamp(0, 15) as i32
        }
        "crafter" => crafter_output(world, pos),
        "creaking_heart" => {
            if state.property("creaking_heart_state") == Some("uprooted") {
                0
            } else {
                block_entity_i64(world, pos, &["output_signal", "OutputSignal"])
                    .unwrap_or(0)
                    .clamp(0, 15) as i32
            }
        }
        "detector_rail" => detector_rail_output(world, pos, state.bool_property("powered")),
        "jukebox" => block_entity_i64(world, pos, &["comparator_output", "ComparatorOutput"])
            .map_or_else(
                || block_entity::jukebox_output(world, pos),
                |output| output.clamp(0, 15) as i32,
            ),
        "lectern" => block_entity_i64(world, pos, &["comparator_output"]).map_or_else(
            || block_entity::lectern_output(world, pos, state),
            |output| output.clamp(0, 15) as i32,
        ),
        "sculk_sensor" | "calibrated_sculk_sensor" => {
            if state.property("sculk_sensor_phase") == Some("active") {
                block_entity_i64(
                    world,
                    pos,
                    &["last_vibration_frequency", "LastVibrationFrequency"],
                )
                .unwrap_or(0)
                .clamp(0, 15) as i32
            } else {
                0
            }
        }
        value if value.ends_with("_shelf") && value != "chiseled_bookshelf" => {
            shelf_output(world, pos, state, direction)
        }
        "decorated_pot" => block_entity::decorated_pot_output(world, pos),
        value if is_chest(value) => chest_output(registry, world, pos, state),
        value if is_inventory_output_block(value) => {
            i32::from(container_signal(world, pos).unwrap_or(0))
        }
        _ => return None,
    };
    Some(clamp_signal(i64::from(output)))
}

pub(super) fn has_analog_output(state: &crate::StateDefinition) -> bool {
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    matches!(
        path,
        "cauldron"
            | "water_cauldron"
            | "powder_snow_cauldron"
            | "lava_cauldron"
            | "composter"
            | "cake"
            | "bee_nest"
            | "beehive"
            | "end_portal_frame"
            | "respawn_anchor"
            | "chiseled_bookshelf"
            | "command_block"
            | "chain_command_block"
            | "repeating_command_block"
            | "crafter"
            | "creaking_heart"
            | "detector_rail"
            | "jukebox"
            | "lectern"
            | "sculk_sensor"
            | "calibrated_sculk_sensor"
            | "decorated_pot"
    ) || path == "candle_cake"
        || path.ends_with("_candle_cake")
        || path.ends_with("_bulb")
        || path.ends_with("copper_golem_statue")
        || path.ends_with("_shelf")
        || is_chest(path)
        || is_inventory_output_block(path)
}

fn copper_golem_pose_output(pose: &str) -> i32 {
    match pose {
        "standing" => 1,
        "sitting" => 2,
        "running" => 3,
        "star" => 4,
        _ => 0,
    }
}

fn crafter_output(world: &SparseWorld, pos: BlockPos) -> i32 {
    let Some(data) = world.block_entity(pos) else {
        return 0;
    };
    let occupied = inventory_slots(&data.fields);
    let disabled = data
        .fields
        .get("disabled_slots")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_u64)
        .filter(|slot| *slot < 9 && !occupied.contains(slot))
        .collect::<BTreeSet<_>>();
    i32::try_from(occupied.len() + disabled.len())
        .unwrap_or(9)
        .clamp(0, 9)
}

fn shelf_output(
    world: &SparseWorld,
    pos: BlockPos,
    state: &crate::StateDefinition,
    direction: Direction,
) -> i32 {
    if state.direction_property("facing").map(Direction::opposite) != Some(direction) {
        return 0;
    }
    let Some(data) = world.block_entity(pos) else {
        return 0;
    };
    inventory_slots(&data.fields)
        .into_iter()
        .filter(|slot| *slot < 3)
        .fold(0, |output, slot| output | 1 << slot)
}

fn chest_output(
    registry: &Java26Registry,
    world: &SparseWorld,
    pos: BlockPos,
    state: &crate::StateDefinition,
) -> i32 {
    if state.property("type").is_none_or(|kind| kind == "single") {
        return i32::from(container_signal(world, pos).unwrap_or(0));
    }
    let Some(partner_direction) = chest_partner_direction(state) else {
        return 0;
    };
    let partner_pos = pos.relative(partner_direction);
    let Some(partner) = registry.state(world.get_block(partner_pos)) else {
        return 0;
    };
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let partner_path = partner
        .name
        .strip_prefix("minecraft:")
        .unwrap_or(&partner.name);
    if !chests_connect(path, partner_path)
        || partner.direction_property("facing") != state.direction_property("facing")
        || partner.property("type") == state.property("type")
    {
        return 0;
    }
    i32::from(container_signal_for_positions(world, &[pos, partner_pos]).unwrap_or(0))
}

fn chest_partner_direction(state: &crate::StateDefinition) -> Option<Direction> {
    let facing = state.direction_property("facing")?;
    match state.property("type")? {
        "left" => Some(facing.clockwise()),
        "right" => Some(facing.counter_clockwise()),
        _ => None,
    }
}

fn detector_rail_output(world: &SparseWorld, pos: BlockPos, powered: bool) -> i32 {
    if !powered {
        return 0;
    }
    let entities = world
        .entity_ids_in_aabb(
            [pos.x as f64 + 0.2, pos.y as f64, pos.z as f64 + 0.2],
            [pos.x as f64 + 0.8, pos.y as f64 + 0.8, pos.z as f64 + 0.8],
        )
        .into_iter()
        .filter_map(|id| world.entity(id))
        .collect::<Vec<_>>();
    if let Some(command_cart) = entities
        .iter()
        .find(|entity| entity.kind == "minecraft:command_block_minecart")
    {
        return entity_i64(command_cart, &["SuccessCount", "success_count"])
            .unwrap_or(0)
            .clamp(0, 15) as i32;
    }
    entities
        .into_iter()
        .find_map(entity_container_signal)
        .map_or(0, i32::from)
}

fn entity_container_signal(entity: &EntityData) -> Option<u8> {
    let has_inventory = entity.fields.contains_key("inventory")
        || entity.fields.contains_key("capacity")
        || matches!(
            entity.kind.as_str(),
            "minecraft:chest_minecart" | "minecraft:hopper_minecart"
        );
    has_inventory.then(|| container_signal_from_fields(&entity.kind, &entity.fields))
}

fn inventory_slots(fields: &BTreeMap<String, Value>) -> BTreeSet<u64> {
    fields
        .get("inventory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("count").and_then(Value::as_i64).unwrap_or(0) > 0)
        .filter_map(|entry| entry.get("slot").and_then(Value::as_u64))
        .collect()
}

fn block_entity_i64(world: &SparseWorld, pos: BlockPos, keys: &[&str]) -> Option<i64> {
    let data = world.block_entity(pos)?;
    keys.iter()
        .find_map(|key| data.fields.get(*key).and_then(Value::as_i64))
}

fn entity_i64(entity: &EntityData, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| entity.fields.get(*key).and_then(Value::as_i64))
}

fn is_inventory_output_block(path: &str) -> bool {
    path.ends_with("_shulker_box")
        || matches!(
            path,
            "furnace"
                | "blast_furnace"
                | "smoker"
                | "barrel"
                | "brewing_stand"
                | "dropper"
                | "dispenser"
                | "hopper"
                | "shulker_box"
        )
}

fn is_chest(path: &str) -> bool {
    matches!(path, "chest" | "trapped_chest") || path.ends_with("copper_chest")
}

fn chests_connect(path: &str, partner: &str) -> bool {
    path == partner || path.ends_with("copper_chest") && partner.ends_with("copper_chest")
}

fn clamp_signal(output: i64) -> u8 {
    output.clamp(0, 15) as u8
}
