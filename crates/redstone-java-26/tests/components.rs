use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockEntityData, BlockPos, BlockStateId, Direction, EntityData, Probe, ProbeValue,
    RedstoneMode, Simulation, SimulationConfig, SparseWorld, TraceKind,
};
use redstone_java_26::{Java26Registry, Java26Rules, StateResolver};

fn state(
    registry: &mut Java26Registry,
    name: &str,
    properties: &[(&str, &str)],
) -> BlockStateId {
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

fn wire(registry: &mut Java26Registry, power: u8) -> BlockStateId {
    state(
        registry,
        "minecraft:redstone_wire",
        &[
            ("power", &power.to_string()),
            ("north", "side"),
            ("east", "side"),
            ("south", "side"),
            ("west", "side"),
        ],
    )
}

fn container(kind: &str, slots: &[(&str, i64)]) -> BlockEntityData {
    let inventory = slots
        .iter()
        .enumerate()
        .map(|(slot, (item_id, count))| {
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
                serde_json::Value::from(slots.iter().map(|(_, count)| *count).sum::<i64>()),
            ),
            ("slot_count".to_owned(), serde_json::Value::from(9)),
            ("capacity".to_owned(), serde_json::Value::from(576)),
        ]),
    }
}

#[tokio::test]
async fn redstone_block_powers_a_wire_in_both_modes() {
    for mode in [RedstoneMode::Default, RedstoneMode::Experimental] {
        let mut registry = Java26Registry::new();
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let wire = wire(&mut registry, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, source).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), wire).unwrap();
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(
            rules,
            world,
            SimulationConfig {
                mode,
                seed: 42,
                ..SimulationConfig::default()
            },
        )
        .await
        .unwrap();

        simulation.initialize().await.unwrap();
        let state_id = simulation.world().get_block(BlockPos::new(1, 0, 0));
        assert_eq!(
            simulation.rules().registry().state(state_id).unwrap().property("power"),
            Some("15")
        );
    }
}

#[tokio::test]
async fn default_wire_does_not_power_itself_through_a_conductor() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let powered_wire = wire(&mut registry, 15);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, source).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), powered_wire).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation
        .step_with_actions(&[Action::BreakBlock {
            pos: BlockPos::ZERO,
        }])
        .await
        .unwrap();

    let wire_id = simulation.world().get_block(BlockPos::new(1, 0, 0));
    assert_eq!(
        simulation.rules().registry().state(wire_id).unwrap().property("power"),
        Some("0")
    );
}

#[tokio::test]
async fn repeater_waits_for_its_configured_delay_and_outputs_forward() {
    let mut registry = Java26Registry::new();
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let repeater = state(
        &mut registry,
        "minecraft:repeater",
        &[
            ("facing", "north"),
            ("delay", "1"),
            ("locked", "false"),
            ("powered", "false"),
        ],
    );
    let output_wire = wire(&mut registry, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::new(0, 0, -1), source).unwrap();
    world.set_block(BlockPos::ZERO, repeater).unwrap();
    world.set_block(BlockPos::new(0, 0, 1), output_wire).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().await.unwrap();
    simulation.step().await.unwrap();
    let state_id = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(state_id).unwrap().property("powered"),
        Some("false")
    );

    let delta = simulation.step().await.unwrap();
    assert_eq!(
        delta.probes[0].value,
        ProbeValue::Integer(15),
    );
    let wire_id = simulation.world().get_block(BlockPos::new(0, 0, 1));
    assert_eq!(
        simulation.rules().registry().state(wire_id).unwrap().property("power"),
        Some("15")
    );
}

#[tokio::test]
async fn comparator_reads_explicit_block_entity_output_from_a_lectern() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "north"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[("facing", "north"), ("has_book", "true"), ("powered", "false")],
    );
    let input = BlockPos::new(0, 0, -1);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(input, lectern).unwrap();
    world.set_block_entity(
        input,
        BlockEntityData {
            kind: "minecraft:lectern".to_owned(),
            fields: BTreeMap::from([(
                "comparator_output".to_owned(),
                serde_json::Value::from(7),
            )]),
        },
    );
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::North),
        },
    );
    simulation.initialize().await.unwrap();

    simulation.step().await.unwrap();
    let delta = simulation.step().await.unwrap();

    assert_eq!(delta.probes[0].value, ProbeValue::Integer(7));
}

#[tokio::test]
async fn observer_emits_a_two_tick_pulse_after_observed_change() {
    let mut registry = Java26Registry::new();
    let observer = state(
        &mut registry,
        "minecraft:observer",
        &[("facing", "north"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, observer).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation
        .step_with_actions(&[Action::SetBlock {
            pos: BlockPos::new(0, 0, -1),
            state: stone,
        }])
        .await
        .unwrap();
    simulation.step().await.unwrap();
    simulation.step().await.unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(powered).unwrap().property("powered"),
        Some("true")
    );
    simulation.step().await.unwrap();
    simulation.step().await.unwrap();
    let unpowered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(unpowered).unwrap().property("powered"),
        Some("false")
    );
}

#[tokio::test]
async fn hopper_pulls_one_item_and_starts_an_eight_tick_cooldown() {
    let mut registry = Java26Registry::new();
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "east")],
    );
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[("facing", "north"), ("type", "single"), ("waterlogged", "false")],
    );
    let hopper_pos = BlockPos::ZERO;
    let source_pos = hopper_pos.relative(Direction::Up);
    let target_pos = hopper_pos.relative(Direction::East);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(hopper_pos, hopper).unwrap();
    world.set_block(source_pos, chest).unwrap();
    world.set_block(target_pos, chest).unwrap();
    world.set_block_entity(hopper_pos, container("minecraft:hopper", &[]));
    world.set_block_entity(source_pos, container("minecraft:chest", &[("minecraft:stone", 2)]));
    world.set_block_entity(target_pos, container("minecraft:chest", &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();

    assert_eq!(simulation.world().block_entity(hopper_pos).unwrap().fields["item_count"], 1);
    assert_eq!(simulation.world().block_entity(source_pos).unwrap().fields["item_count"], 1);
    assert_eq!(simulation.world().block_entity(target_pos).unwrap().fields["item_count"], 0);
    assert_eq!(simulation.world().block_entity(hopper_pos).unwrap().fields["cooldown"], 8);
}

#[tokio::test]
async fn powered_dropper_transfers_into_the_facing_container_after_four_ticks() {
    let mut registry = Java26Registry::new();
    let dropper = state(
        &mut registry,
        "minecraft:dropper",
        &[("facing", "east"), ("triggered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let chest = state(
        &mut registry,
        "minecraft:chest",
        &[("facing", "north"), ("type", "single"), ("waterlogged", "false")],
    );
    let target = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, dropper).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    world.set_block(target, chest).unwrap();
    world.set_block_entity(
        BlockPos::ZERO,
        container("minecraft:dropper", &[("minecraft:redstone", 2)]),
    );
    world.set_block_entity(target, container("minecraft:chest", &[]));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.run_until(redstone_core::GameTick(5)).await.unwrap();

    assert_eq!(simulation.world().block_entity(BlockPos::ZERO).unwrap().fields["item_count"], 1);
    assert_eq!(simulation.world().block_entity(target).unwrap().fields["item_count"], 1);
}

#[tokio::test]
async fn dispenser_consumes_registered_items_but_preserves_unknown_items() {
    for (item_id, consumed, entity_kind) in [
        ("minecraft:arrow", true, Some("minecraft:arrow")),
        ("minecraft:stone", false, None),
    ] {
        let mut registry = Java26Registry::new();
        let dispenser = state(
            &mut registry,
            "minecraft:dispenser",
            &[("facing", "east"), ("triggered", "false")],
        );
        let source = state(&mut registry, "minecraft:redstone_block", &[]);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::ZERO, dispenser).unwrap();
        world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
        world.set_block_entity(BlockPos::ZERO, container("minecraft:dispenser", &[(item_id, 1)]));
        let rules = Java26Rules::new(registry);
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .await
            .unwrap();
        simulation.initialize().await.unwrap();

        simulation.run_until(redstone_core::GameTick(5)).await.unwrap();

        assert_eq!(
            simulation.world().block_entity(BlockPos::ZERO).unwrap().fields["item_count"],
            i64::from(!consumed)
        );
        assert_eq!(
            simulation
                .world()
                .entities()
                .find(|(_, entity)| entity.kind == entity_kind.unwrap_or_default())
                .map(|(_, entity)| entity.kind.as_str()),
            entity_kind
        );
        assert_eq!(
            simulation
                .trace()
                .events()
                .iter()
                .any(|event| matches!(event.kind, TraceKind::UnsupportedTrigger { .. })),
            !consumed
        );
    }
}

#[tokio::test]
async fn crafter_emits_output_and_clears_its_crafting_pulse() {
    let mut registry = Java26Registry::new();
    let crafter = state(
        &mut registry,
        "minecraft:crafter",
        &[
            ("crafting", "false"),
            ("orientation", "east_up"),
            ("triggered", "false"),
        ],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, crafter).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    let mut data = container("minecraft:crafter", &[("minecraft:stone", 1)]);
    data.fields.insert(
        "output_item_id".to_owned(),
        serde_json::Value::String("minecraft:redstone".to_owned()),
    );
    data.fields.insert("output_count".to_owned(), serde_json::Value::from(2));
    world.set_block_entity(BlockPos::ZERO, data);
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.run_until(redstone_core::GameTick(6)).await.unwrap();

    assert_eq!(simulation.world().block_entity(BlockPos::ZERO).unwrap().fields["item_count"], 0);
    assert_eq!(simulation.world().entities().count(), 1);
    let output = simulation.world().entities().next().unwrap().1;
    assert_eq!(output.fields["item_id"], "minecraft:redstone");
    assert_eq!(output.fields["item_count"], 2);
    let final_state = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(final_state).unwrap().property("crafting"),
        Some("false")
    );
}

#[tokio::test]
async fn floor_button_notifies_consumers_next_to_its_strongly_powered_support() {
    let mut registry = Java26Registry::new();
    let button = state(
        &mut registry,
        "minecraft:oak_button",
        &[("face", "floor"), ("facing", "north"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let button_pos = BlockPos::new(0, 1, 0);
    let lamp_pos = BlockPos::new(1, 0, 0);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(button_pos, button).unwrap();
    world.set_block(BlockPos::ZERO, stone).unwrap();
    world.set_block(lamp_pos, lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation
        .step_with_actions(&[Action::PressButton { pos: button_pos }])
        .await
        .unwrap();

    let lit = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation.rules().registry().state(lit).unwrap().property("lit"),
        Some("true")
    );
    simulation.run_until(redstone_core::GameTick(35)).await.unwrap();
    let unlit = simulation.world().get_block(lamp_pos);
    assert_eq!(
        simulation.rules().registry().state(unlit).unwrap().property("lit"),
        Some("false")
    );
}

#[tokio::test]
async fn floor_sources_strongly_power_the_block_below() {
    let mut registry = Java26Registry::new();
    let pressure_plate = state(
        &mut registry,
        "minecraft:oak_pressure_plate",
        &[("powered", "true")],
    );
    let detector_rail = state(
        &mut registry,
        "minecraft:detector_rail",
        &[("powered", "true"), ("shape", "north_south"), ("waterlogged", "false")],
    );
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[("facing", "north"), ("has_book", "true"), ("powered", "true")],
    );
    let trapped_chest = state(
        &mut registry,
        "minecraft:trapped_chest",
        &[("facing", "north"), ("type", "single"), ("waterlogged", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let trapped_chest_data = BlockEntityData {
        kind: "minecraft:trapped_chest".to_owned(),
        fields: BTreeMap::from([("open_count".to_owned(), serde_json::Value::from(1))]),
    };

    for (source, block_entity) in [
        (pressure_plate, None),
        (detector_rail, None),
        (lectern, None),
        (trapped_chest, Some(trapped_chest_data)),
    ] {
        let source_pos = BlockPos::new(0, 1, 0);
        let lamp_pos = BlockPos::new(1, 0, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(source_pos, source).unwrap();
        world.set_block(BlockPos::ZERO, stone).unwrap();
        world.set_block(lamp_pos, lamp).unwrap();
        if let Some(data) = block_entity {
            world.set_block_entity(source_pos, data);
        }
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .await
            .unwrap();

        simulation.initialize().await.unwrap();

        let lit = simulation.world().get_block(lamp_pos);
        assert_eq!(
            simulation.rules().registry().state(lit).unwrap().property("lit"),
            Some("true")
        );
    }
}

#[tokio::test]
async fn weak_only_sources_and_lit_copper_bulbs_do_not_power_through_a_conductor() {
    let mut registry = Java26Registry::new();
    let redstone_block = state(&mut registry, "minecraft:redstone_block", &[]);
    let target = state(&mut registry, "minecraft:target", &[("power", "15")]);
    let daylight_detector = state(
        &mut registry,
        "minecraft:daylight_detector",
        &[("inverted", "false"), ("power", "15")],
    );
    let copper_bulb = state(
        &mut registry,
        "minecraft:copper_bulb",
        &[("lit", "true"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);

    for source in [redstone_block, target, daylight_detector, copper_bulb] {
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
        world.set_block(BlockPos::ZERO, stone).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), lamp).unwrap();
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .await
            .unwrap();

        simulation.initialize().await.unwrap();

        let unlit = simulation.world().get_block(BlockPos::new(1, 0, 0));
        assert_eq!(
            simulation.rules().registry().state(unlit).unwrap().property("lit"),
            Some("false")
        );
    }
}

#[tokio::test]
async fn pistons_and_hoppers_do_not_relay_strong_power() {
    let mut registry = Java26Registry::new();
    let source = state(
        &mut registry,
        "minecraft:lever",
        &[("face", "floor"), ("facing", "north"), ("powered", "true")],
    );
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "up")],
    );
    let hopper = state(
        &mut registry,
        "minecraft:hopper",
        &[("enabled", "true"), ("facing", "down")],
    );
    let redstone_lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);

    for machine in [piston, hopper] {
        let output_pos = BlockPos::new(2, 0, 0);
        let mut world = SparseWorld::new(registry.air_state());
        world.set_block(BlockPos::new(1, 1, 0), source).unwrap();
        world.set_block(BlockPos::new(1, 0, 0), machine).unwrap();
        world.set_block(output_pos, redstone_lamp).unwrap();
        let rules = Java26Rules::new(registry.clone());
        let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
            .await
            .unwrap();

        simulation.initialize().await.unwrap();

        let output = simulation.world().get_block(output_pos);
        assert_eq!(
            simulation.rules().registry().state(output).unwrap().property("lit"),
            Some("false")
        );
    }
}

#[tokio::test]
async fn wooden_pressure_plate_tracks_item_entities_and_releases_after_delay() {
    let mut registry = Java26Registry::new();
    let plate = state(
        &mut registry,
        "minecraft:oak_pressure_plate",
        &[("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, plate).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:item".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::from([
            ("item_id".to_owned(), serde_json::Value::String("minecraft:stone".to_owned())),
            ("item_count".to_owned(), serde_json::Value::from(1)),
            ("age".to_owned(), serde_json::Value::from(5_998)),
        ]),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(powered).unwrap().property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation.rules().registry().state(lit).unwrap().property("lit"),
        Some("true")
    );
    simulation.run_until(redstone_core::GameTick(21)).await.unwrap();
    let released = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(released).unwrap().property("powered"),
        Some("false")
    );
    assert_eq!(simulation.world().entities().count(), 0);
}

#[tokio::test]
async fn detector_rail_responds_only_to_minecarts() {
    let mut registry = Java26Registry::new();
    let rail = state(
        &mut registry,
        "minecraft:detector_rail",
        &[("powered", "false"), ("shape", "north_south"), ("waterlogged", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, rail).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:hopper_minecart".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::new(),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.step().await.unwrap();

    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(powered).unwrap().property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation.rules().registry().state(lit).unwrap().property("lit"),
        Some("true")
    );
}

#[tokio::test]
async fn tripwire_powers_a_facing_hook_when_an_entity_intersects() {
    let mut registry = Java26Registry::new();
    let wire = state(
        &mut registry,
        "minecraft:tripwire",
        &[
            ("attached", "true"),
            ("disarmed", "false"),
            ("east", "true"),
            ("north", "false"),
            ("powered", "false"),
            ("south", "false"),
            ("west", "true"),
        ],
    );
    let hook = state(
        &mut registry,
        "minecraft:tripwire_hook",
        &[("attached", "true"), ("facing", "west"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, wire).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), hook).unwrap();
    world.set_block(BlockPos::new(2, 0, 0), stone).unwrap();
    world.set_block(BlockPos::new(2, 0, 1), lamp).unwrap();
    world.spawn_entity(EntityData {
        kind: "minecraft:generic_collision".to_owned(),
        position: [0.5, 0.1, 0.5],
        fields: BTreeMap::new(),
    });
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.step().await.unwrap();

    let hook = simulation.world().get_block(BlockPos::new(1, 0, 0));
    assert_eq!(
        simulation.rules().registry().state(hook).unwrap().property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(2, 0, 1));
    assert_eq!(
        simulation.rules().registry().state(lit).unwrap().property("lit"),
        Some("true")
    );
}

#[tokio::test]
async fn lectern_emits_a_two_tick_use_pulse() {
    let mut registry = Java26Registry::new();
    let lectern = state(
        &mut registry,
        "minecraft:lectern",
        &[("facing", "north"), ("has_book", "true"), ("powered", "false")],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let lamp = state(&mut registry, "minecraft:redstone_lamp", &[("lit", "false")]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, lectern).unwrap();
    world.set_block(BlockPos::new(0, -1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), lamp).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.apply(Action::UseBlock { pos: BlockPos::ZERO }).await.unwrap();
    let powered = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(powered).unwrap().property("powered"),
        Some("true")
    );
    let lit = simulation.world().get_block(BlockPos::new(1, -1, 0));
    assert_eq!(
        simulation.rules().registry().state(lit).unwrap().property("lit"),
        Some("true")
    );
    simulation.run_until(redstone_core::GameTick(2)).await.unwrap();
    let released = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(released).unwrap().property("powered"),
        Some("false")
    );
}

#[tokio::test]
async fn piston_moves_slime_branches_without_sticking_to_honey() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let slime = state(&mut registry, "minecraft:slime_block", &[]);
    let honey = state(&mut registry, "minecraft:honey_block", &[]);
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), slime).unwrap();
    world.set_block(BlockPos::new(1, 1, 0), stone).unwrap();
    world.set_block(BlockPos::new(1, -1, 0), honey).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.step().await.unwrap();

    let moving_slime = simulation
        .world()
        .block_entity(BlockPos::new(2, 0, 0))
        .unwrap();
    assert_eq!(moving_slime.kind, "minecraft:moving_piston");
    assert_eq!(moving_slime.fields["moved_state"], slime.0);
    let moving_stone = simulation
        .world()
        .block_entity(BlockPos::new(2, 1, 0))
        .unwrap();
    assert_eq!(moving_stone.fields["moved_state"], stone.0);
    let moving_order = simulation
        .world()
        .block_entities()
        .filter(|(_, data)| data.kind == "minecraft:moving_piston")
        .map(|(pos, data)| (*pos, data.fields["source"].as_bool().unwrap()))
        .collect::<Vec<_>>();
    assert!(moving_order.last().is_some_and(|(_, source)| *source));
    assert!(moving_order[..moving_order.len() - 1]
        .iter()
        .all(|(_, source)| !source));
    simulation.run_until(redstone_core::GameTick(4)).await.unwrap();

    assert_eq!(simulation.world().get_block(BlockPos::new(2, 0, 0)), slime);
    assert_eq!(simulation.world().get_block(BlockPos::new(2, 1, 0)), stone);
    assert_eq!(simulation.world().get_block(BlockPos::new(1, -1, 0)), honey);
    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            event.kind,
            TraceKind::BlockEventExecuted { param_a: 0, .. }
        )
    }));
}

#[tokio::test]
async fn piston_rejects_a_sticky_structure_larger_than_twelve_blocks() {
    let mut registry = Java26Registry::new();
    let piston = state(
        &mut registry,
        "minecraft:piston",
        &[("extended", "false"), ("facing", "east")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let slime = state(&mut registry, "minecraft:slime_block", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, piston).unwrap();
    world.set_block(BlockPos::new(-1, 0, 0), source).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), slime).unwrap();
    for y in 1..=12 {
        world.set_block(BlockPos::new(1, y, 0), slime).unwrap();
    }
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();

    simulation.step().await.unwrap();

    let piston_state = simulation.world().get_block(BlockPos::ZERO);
    assert_eq!(
        simulation.rules().registry().state(piston_state).unwrap().property("extended"),
        Some("false")
    );
    assert_eq!(simulation.world().get_block(BlockPos::new(1, 0, 0)), slime);
}
