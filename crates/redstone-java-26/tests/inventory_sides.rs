use std::collections::BTreeMap;

use redstone_core::{
    BlockEntityData, BlockPos, BlockStateId, Simulation, SimulationConfig, SparseWorld,
};
use redstone_java_26::{Java26Registry, Java26Rules, StateResolver};

fn state(registry: &mut Java26Registry, name: &str, properties: &[(&str, &str)]) -> BlockStateId {
    registry
        .resolve_state(
            name,
            &properties
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
}

fn container(kind: &str, slot_count: i64, slots: &[(u32, &str, i64)]) -> BlockEntityData {
    let inventory = slots
        .iter()
        .map(|(slot, item_id, count)| {
            serde_json::json!({
                "slot": slot,
                "item_id": item_id,
                "count": count,
            })
        })
        .collect::<Vec<_>>();
    BlockEntityData {
        kind: kind.to_owned(),
        fields: BTreeMap::from([
            ("inventory".to_owned(), serde_json::Value::Array(inventory)),
            (
                "item_count".to_owned(),
                serde_json::Value::from(slots.iter().map(|(_, _, count)| *count).sum::<i64>()),
            ),
            ("slot_count".to_owned(), serde_json::Value::from(slot_count)),
            (
                "capacity".to_owned(),
                serde_json::Value::from(slot_count * 64),
            ),
        ]),
    }
}

fn slot_item(world: &SparseWorld, pos: BlockPos, slot: u64) -> Option<(&str, i64)> {
    world
        .block_entity(pos)?
        .fields
        .get("inventory")?
        .as_array()?
        .iter()
        .find(|entry| entry["slot"].as_u64() == Some(slot))
        .and_then(|entry| Some((entry["item_id"].as_str()?, entry["count"].as_i64()?)))
}

#[tokio::test]
async fn hoppers_use_the_furnace_slots_exposed_by_each_face() {
    let mut registry = Java26Registry::new();
    let hopper_east = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let hopper_down = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let furnace = state(
        &mut registry,
        "minecraft:furnace",
        &[("facing", "north"), ("lit", "false")],
    );
    let side_hopper = BlockPos::ZERO;
    let side_furnace = BlockPos::new(1, 0, 0);
    let top_hopper = BlockPos::new(3, 1, 0);
    let top_furnace = BlockPos::new(3, 0, 0);
    let rejected_hopper = BlockPos::new(6, 0, 0);
    let rejected_furnace = BlockPos::new(7, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for (pos, state) in [
        (side_hopper, hopper_east),
        (side_furnace, furnace),
        (top_hopper, hopper_down),
        (top_furnace, furnace),
        (rejected_hopper, hopper_east),
        (rejected_furnace, furnace),
    ] {
        world.set_block(pos, state).unwrap();
    }
    world.set_block_entity(
        side_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:coal", 1)]),
    );
    world.set_block_entity(side_furnace, container("minecraft:furnace", 3, &[]));
    world.set_block_entity(
        top_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:cobblestone", 1)]),
    );
    world.set_block_entity(top_furnace, container("minecraft:furnace", 3, &[]));
    world.set_block_entity(
        rejected_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:cobblestone", 1)]),
    );
    world.set_block_entity(rejected_furnace, container("minecraft:furnace", 3, &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();

    assert_eq!(
        slot_item(simulation.world(), side_furnace, 1),
        Some(("minecraft:coal", 1))
    );
    assert_eq!(
        slot_item(simulation.world(), top_furnace, 0),
        Some(("minecraft:cobblestone", 1))
    );
    assert_eq!(
        slot_item(simulation.world(), rejected_hopper, 0),
        Some(("minecraft:cobblestone", 1))
    );
    assert_eq!(slot_item(simulation.world(), rejected_furnace, 1), None);
}

#[tokio::test]
async fn brewing_stands_expose_ingredient_fuel_and_output_slots_by_face() {
    let mut registry = Java26Registry::new();
    let hopper_east = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let hopper_down = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "north")],
    );
    let brewing = state(
        &mut registry,
        "minecraft:brewing_stand",
        &[
            ("has_bottle_0", "false"),
            ("has_bottle_1", "false"),
            ("has_bottle_2", "false"),
        ],
    );
    let top_hopper = BlockPos::new(0, 1, 0);
    let top_brewing = BlockPos::ZERO;
    let side_hopper = BlockPos::new(3, 0, 0);
    let side_brewing = BlockPos::new(4, 0, 0);
    let bottom_hopper = BlockPos::new(7, 0, 0);
    let bottom_brewing = BlockPos::new(7, 1, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for (pos, state) in [
        (top_hopper, hopper_down),
        (top_brewing, brewing),
        (side_hopper, hopper_east),
        (side_brewing, brewing),
        (bottom_hopper, hopper),
        (bottom_brewing, brewing),
    ] {
        world.set_block(pos, state).unwrap();
    }
    world.set_block_entity(
        top_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:nether_wart", 1)]),
    );
    world.set_block_entity(top_brewing, container("minecraft:brewing_stand", 5, &[]));
    world.set_block_entity(
        side_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:blaze_powder", 1)]),
    );
    world.set_block_entity(side_brewing, container("minecraft:brewing_stand", 5, &[]));
    world.set_block_entity(bottom_hopper, container("minecraft:hopper", 5, &[]));
    world.set_block_entity(
        bottom_brewing,
        container(
            "minecraft:brewing_stand",
            5,
            &[(3, "minecraft:nether_wart", 1)],
        ),
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();

    assert_eq!(
        slot_item(simulation.world(), top_brewing, 3),
        Some(("minecraft:nether_wart", 1))
    );
    assert_eq!(
        slot_item(simulation.world(), side_brewing, 4),
        Some(("minecraft:blaze_powder", 1))
    );
    assert_eq!(
        slot_item(simulation.world(), bottom_brewing, 3),
        Some(("minecraft:nether_wart", 1))
    );
    assert_eq!(
        simulation
            .world()
            .block_entity(bottom_hopper)
            .unwrap()
            .fields["item_count"],
        0
    );
}

#[tokio::test]
async fn hopper_insertion_skips_disabled_crafter_slots() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let crafter = state(
        &mut registry,
        "minecraft:crafter",
        &[
            ("crafting", "false"),
            ("orientation", "east_up"),
            ("triggered", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let crafter_pos = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(crafter_pos, crafter).unwrap();
    world.set_block_entity(
        hopper_pos,
        container("minecraft:hopper", 5, &[(0, "minecraft:stone", 1)]),
    );
    let mut crafter_data = container("minecraft:crafter", 9, &[]);
    crafter_data.fields.insert(
        "disabled_slots".to_owned(),
        serde_json::json!([0, 2, 3, 4, 5, 6, 7, 8]),
    );
    world.set_block_entity(crafter_pos, crafter_data);
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();

    assert_eq!(slot_item(simulation.world(), crafter_pos, 0), None);
    assert_eq!(
        slot_item(simulation.world(), crafter_pos, 1),
        Some(("minecraft:stone", 1))
    );
}

#[tokio::test]
async fn dropper_preserves_a_shulker_box_rejected_by_an_existing_container() {
    let mut registry = Java26Registry::new();
    let dropper = state(
        &mut registry,
        "minecraft:dropper",
        &[("facing", "east"), ("triggered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let shulker = state(&mut registry, "minecraft:shulker_box", &[("facing", "up")]);
    let target = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, dropper).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    world.set_block(target, shulker).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        container(
            "minecraft:dropper",
            9,
            &[(0, "minecraft:blue_shulker_box", 1)],
        ),
    );
    world.set_block_entity(target, container("minecraft:shulker_box", 27, &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation
        .run_until(redstone_core::GameTick(5))
        .await
        .unwrap();

    assert_eq!(
        slot_item(simulation.world(), BlockPos::ZERO, 0),
        Some(("minecraft:blue_shulker_box", 1))
    );
    assert_eq!(simulation.world().entities().count(), 0);
    assert_eq!(
        simulation.world().block_entity(target).unwrap().fields["item_count"],
        0
    );
}
