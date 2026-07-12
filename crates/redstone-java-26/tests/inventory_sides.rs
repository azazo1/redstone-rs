use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockEntityData, BlockPos, BlockStateId, Direction, EntityData, EntityId, Simulation,
    SimulationConfig, SparseWorld,
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

fn item_count(world: &SparseWorld, pos: BlockPos) -> i64 {
    world.block_entity(pos).unwrap().fields["item_count"]
        .as_i64()
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

#[test]
fn hoppers_use_the_furnace_slots_exposed_by_each_face() {
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
        .unwrap();

    simulation.step().unwrap();

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

#[test]
fn brewing_stands_expose_ingredient_fuel_and_output_slots_by_face() {
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
        .unwrap();

    simulation.step().unwrap();

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

#[test]
fn hopper_insertion_skips_disabled_crafter_slots() {
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
        .unwrap();

    simulation.step().unwrap();

    assert_eq!(slot_item(simulation.world(), crafter_pos, 0), None);
    assert_eq!(
        slot_item(simulation.world(), crafter_pos, 1),
        Some(("minecraft:stone", 1))
    );
}

#[test]
fn hopper_pushes_and_pulls_during_the_same_tick() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "south"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let source_pos = hopper_pos.relative(Direction::Up);
    let target_pos = hopper_pos.relative(Direction::East);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(source_pos, chest).unwrap();
    world.set_block(target_pos, chest).unwrap();
    world.set_block_entity(
        hopper_pos,
        container("minecraft:hopper", 5, &[(0, "minecraft:stone", 1)]),
    );
    world.set_block_entity(
        source_pos,
        container("minecraft:chest", 27, &[(0, "minecraft:iron_ingot", 1)]),
    );
    world.set_block_entity(target_pos, container("minecraft:chest", 27, &[]));
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(slot_item(simulation.world(), target_pos, 0), Some(("minecraft:stone", 1)));
    assert_eq!(slot_item(simulation.world(), hopper_pos, 0), Some(("minecraft:iron_ingot", 1)));
    assert_eq!(item_count(simulation.world(), source_pos), 0);
}

#[test]
fn powered_hopper_continues_to_reduce_its_cooldown() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "false"), ("facing", "down")],
    );
    let power = state(&mut registry, "minecraft:redstone_block", &[]);
    let hopper_pos = BlockPos::ZERO;
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world
        .set_block(hopper_pos.relative(Direction::East), power)
        .unwrap();
    let mut data = container("minecraft:hopper", 5, &[]);
    data.fields
        .insert("cooldown".to_owned(), serde_json::Value::from(3));
    world.set_block_entity(hopper_pos, data);
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(
        simulation.world().block_entity(hopper_pos).unwrap().fields["cooldown"],
        2
    );
}

#[test]
fn hopper_enabled_state_changes_during_neighbor_updates() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let power = state(&mut registry, "minecraft:redstone_block", &[]);
    let hopper_pos = BlockPos::ZERO;
    let power_pos = hopper_pos.relative(Direction::East);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block_entity(hopper_pos, container("minecraft:hopper", 5, &[]));
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: power_pos,
            state: power,
        }])
        .unwrap();

    let state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(hopper_pos))
        .unwrap();
    assert_eq!(state.property("enabled"), Some("false"));
}

#[test]
fn source_container_prevents_fallback_to_item_entities() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "south"),
            ("type", "single"),
            ("waterlogged", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let source_pos = hopper_pos.relative(Direction::Up);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(source_pos, chest).unwrap();
    world.set_block_entity(hopper_pos, container("minecraft:hopper", 5, &[]));
    world.set_block_entity(source_pos, container("minecraft:chest", 27, &[]));
    world
        .spawn_entity_with_id(
            EntityId(10),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [0.5, 1.1, 0.5],
                fields: BTreeMap::from([
                    (
                        "item_id".to_owned(),
                        serde_json::Value::String("minecraft:diamond".to_owned()),
                    ),
                    ("item_count".to_owned(), serde_json::Value::from(3)),
                ]),
            },
        )
        .unwrap();
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(item_count(simulation.world(), hopper_pos), 0);
    assert_eq!(simulation.world().entity(EntityId(10)).unwrap().fields["item_count"], 3);
}

#[test]
fn hopper_absorbs_complete_and_partial_item_stacks() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, hopper).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        container(
            "minecraft:hopper",
            5,
            &[
                (0, "minecraft:stone", 63),
                (1, "minecraft:dirt", 64),
                (2, "minecraft:dirt", 64),
                (3, "minecraft:dirt", 64),
                (4, "minecraft:dirt", 64),
            ],
        ),
    );
    world
        .spawn_entity_with_id(
            EntityId(11),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [0.5, 1.1, 0.5],
                fields: BTreeMap::from([
                    (
                        "item_id".to_owned(),
                        serde_json::Value::String("minecraft:stone".to_owned()),
                    ),
                    ("item_count".to_owned(), serde_json::Value::from(3)),
                ]),
            },
        )
        .unwrap();
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(slot_item(simulation.world(), BlockPos::ZERO, 0), Some(("minecraft:stone", 64)));
    assert_eq!(simulation.world().entity(EntityId(11)).unwrap().fields["item_count"], 2);
    assert_eq!(
        simulation.world().block_entity(BlockPos::ZERO).unwrap().fields["cooldown"],
        0
    );
}

#[test]
fn full_block_above_hopper_blocks_periodic_item_absorption() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, hopper).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), stone).unwrap();
    world.set_block_entity(BlockPos::ZERO, container("minecraft:hopper", 5, &[]));
    world
        .spawn_entity_with_id(
            EntityId(12),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [0.5, 1.7, 0.5],
                fields: BTreeMap::from([
                    (
                        "item_id".to_owned(),
                        serde_json::Value::String("minecraft:stone".to_owned()),
                    ),
                    ("item_count".to_owned(), serde_json::Value::from(1)),
                ]),
            },
        )
        .unwrap();
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(item_count(simulation.world(), BlockPos::ZERO), 0);
    assert!(simulation.world().entity(EntityId(12)).is_some());
}

#[test]
fn hopper_uses_double_chest_and_container_entity_targets() {
    let mut registry = Java26Registry::new();
    let hopper_east = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let chest_right = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "south"),
            ("type", "right"),
            ("waterlogged", "false"),
        ],
    );
    let chest_left = state(
        &mut registry,
        "minecraft:chest",
        &[
            ("facing", "south"),
            ("type", "left"),
            ("waterlogged", "false"),
        ],
    );
    let hopper_pos = BlockPos::ZERO;
    let right_pos = BlockPos::new(1, 0, 0);
    let left_pos = BlockPos::new(2, 0, 0);
    let entity_hopper_pos = BlockPos::new(5, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper_east).unwrap();
    world.set_block(right_pos, chest_right).unwrap();
    world.set_block(left_pos, chest_left).unwrap();
    world.set_block(entity_hopper_pos, hopper_east).unwrap();
    world.set_block_entity(
        hopper_pos,
        container("minecraft:hopper", 5, &[(0, "minecraft:stone", 1)]),
    );
    let full_right = (0..27)
        .map(|slot| (slot, "minecraft:dirt", 64))
        .collect::<Vec<_>>();
    world.set_block_entity(
        right_pos,
        container("minecraft:chest", 27, &full_right),
    );
    world.set_block_entity(left_pos, container("minecraft:chest", 27, &[]));
    world.set_block_entity(
        entity_hopper_pos,
        container("minecraft:hopper", 5, &[(0, "minecraft:iron_ingot", 1)]),
    );
    world
        .spawn_entity_with_id(
            EntityId(13),
            EntityData {
                kind: "minecraft:chest_minecart".to_owned(),
                position: [6.5, 0.5, 0.5],
                fields: container("minecraft:chest_minecart", 27, &[]).fields,
            },
        )
        .unwrap();
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    assert_eq!(slot_item(simulation.world(), left_pos, 0), Some(("minecraft:stone", 1)));
    assert_eq!(
        simulation.world().entity(EntityId(13)).unwrap().fields["item_count"],
        1
    );
}

#[test]
fn hopper_updates_jukebox_and_chiseled_bookshelf_states() {
    let mut registry = Java26Registry::new();
    let hopper_east = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let jukebox = state(
        &mut registry,
        "minecraft:jukebox",
        &[("has_record", "false")],
    );
    let bookshelf = state(
        &mut registry,
        "minecraft:chiseled_bookshelf",
        &[
            ("facing", "north"),
            ("slot_0_occupied", "false"),
            ("slot_1_occupied", "false"),
            ("slot_2_occupied", "false"),
            ("slot_3_occupied", "false"),
            ("slot_4_occupied", "false"),
            ("slot_5_occupied", "false"),
        ],
    );
    let jukebox_hopper = BlockPos::ZERO;
    let jukebox_pos = BlockPos::new(1, 0, 0);
    let bookshelf_hopper = BlockPos::new(4, 0, 0);
    let bookshelf_pos = BlockPos::new(5, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    for (pos, block) in [
        (jukebox_hopper, hopper_east),
        (jukebox_pos, jukebox),
        (bookshelf_hopper, hopper_east),
        (bookshelf_pos, bookshelf),
    ] {
        world.set_block(pos, block).unwrap();
    }
    world.set_block_entity(
        jukebox_hopper,
        container(
            "minecraft:hopper",
            5,
            &[(0, "minecraft:music_disc_cat", 1)],
        ),
    );
    world.set_block_entity(jukebox_pos, container("minecraft:jukebox", 1, &[]));
    world.set_block_entity(
        bookshelf_hopper,
        container("minecraft:hopper", 5, &[(0, "minecraft:book", 1)]),
    );
    world.set_block_entity(
        bookshelf_pos,
        container("minecraft:chiseled_bookshelf", 6, &[]),
    );
    let mut simulation = Simulation::load(
        Java26Rules::new(registry),
        world,
        SimulationConfig::default(),
    )
    .unwrap();

    simulation.step().unwrap();

    let jukebox_state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(jukebox_pos))
        .unwrap();
    assert_eq!(jukebox_state.property("has_record"), Some("true"));
    let bookshelf_state = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(bookshelf_pos))
        .unwrap();
    assert_eq!(bookshelf_state.property("slot_0_occupied"), Some("true"));
    assert_eq!(
        simulation.world().block_entity(bookshelf_pos).unwrap().fields
            ["last_interacted_slot"],
        0
    );
}

#[test]
fn dropper_preserves_a_shulker_box_rejected_by_an_existing_container() {
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
        .unwrap();

    simulation
        .run_until(redstone_core::GameTick(5))
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
