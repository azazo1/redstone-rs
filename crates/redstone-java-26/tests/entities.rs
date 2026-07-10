use std::collections::BTreeMap;

use redstone_core::{
    Action, BlockPos, BlockStateId, Direction, EntityData, EntityId, Probe, ProbeValue, Simulation,
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

#[tokio::test]
async fn entity_actions_move_a_generic_collision_onto_a_pressure_plate() {
    let mut registry = Java26Registry::new();
    let plate = state(
        &mut registry,
        "minecraft:oak_pressure_plate",
        &[("powered", "false")],
    );
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, plate).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.add_probe(
        "entity_x",
        Probe::EntityField {
            id: EntityId(50),
            field: "label".to_owned(),
        },
    );

    simulation
        .step_with_actions(&[Action::SpawnEntity {
            id: Some(EntityId(50)),
            data: EntityData {
                kind: "minecraft:generic_collision".to_owned(),
                position: [3.5, 0.1, 0.5],
                fields: BTreeMap::from([(
                    "label".to_owned(),
                    serde_json::Value::String("test".to_owned()),
                )]),
            },
        }])
        .await
        .unwrap();
    simulation
        .step_with_actions(&[Action::MoveEntity {
            id: EntityId(50),
            position: [0.5, 0.1, 0.5],
        }])
        .await
        .unwrap();

    let plate = simulation
        .rules()
        .registry()
        .state(simulation.world().get_block(BlockPos::ZERO))
        .unwrap();
    assert_eq!(plate.property("powered"), Some("true"));
    let latest = simulation.step().await.unwrap();
    assert_eq!(
        latest.probes[0].value,
        ProbeValue::String("test".to_owned())
    );

    simulation
        .apply(Action::RemoveEntity { id: EntityId(50) })
        .await
        .unwrap();
    assert!(simulation.world().entity(EntityId(50)).is_none());
}

#[tokio::test]
async fn comparator_reads_and_tracks_a_unique_item_frame_through_a_conductor() {
    let mut registry = Java26Registry::new();
    let comparator = state(
        &mut registry,
        "minecraft:comparator",
        &[
            ("facing", "east"),
            ("mode", "compare"),
            ("powered", "false"),
        ],
    );
    let stone = state(&mut registry, "minecraft:stone", &[]);
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, comparator).unwrap();
    world.set_block(BlockPos::new(1, 0, 0), stone).unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.add_probe(
        "output",
        Probe::Signal {
            pos: BlockPos::ZERO,
            direction: Some(Direction::East),
        },
    );
    simulation.initialize().await.unwrap();

    simulation
        .step_with_actions(&[Action::SpawnEntity {
            id: Some(EntityId(7)),
            data: EntityData {
                kind: "minecraft:item_frame".to_owned(),
                position: [2.5, 0.5, 0.5],
                fields: BTreeMap::from([
                    (
                        "facing".to_owned(),
                        serde_json::Value::String("east".to_owned()),
                    ),
                    (
                        "item_id".to_owned(),
                        serde_json::Value::String("minecraft:map".to_owned()),
                    ),
                    ("item_count".to_owned(), serde_json::Value::from(1)),
                    ("rotation".to_owned(), serde_json::Value::from(5)),
                ]),
            },
        }])
        .await
        .unwrap();
    simulation.step().await.unwrap();
    let delta = simulation.step().await.unwrap();
    assert_eq!(delta.probes[0].value, ProbeValue::Integer(6));

    simulation
        .step_with_actions(&[Action::SetEntityField {
            id: EntityId(7),
            field: "rotation".to_owned(),
            value: serde_json::Value::from(7),
        }])
        .await
        .unwrap();
    simulation.step().await.unwrap();
    let delta = simulation.step().await.unwrap();
    assert_eq!(delta.probes[0].value, ProbeValue::Integer(8));
}

#[tokio::test]
async fn hopper_minecart_absorbs_one_item_per_entity_tick() {
    let registry = Java26Registry::new();
    let mut world = SparseWorld::new(registry.air_state());
    world
        .spawn_entity_with_id(
            EntityId(1),
            EntityData {
                kind: "minecraft:hopper_minecart".to_owned(),
                position: [0.5, 0.0, 0.5],
                fields: BTreeMap::from([
                    ("inventory".to_owned(), serde_json::Value::Array(Vec::new())),
                    ("capacity".to_owned(), serde_json::Value::from(320)),
                    ("enabled".to_owned(), serde_json::Value::Bool(true)),
                ]),
            },
        )
        .unwrap();
    world
        .spawn_entity_with_id(
            EntityId(2),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [0.5, 0.1, 0.5],
                fields: BTreeMap::from([
                    (
                        "item_id".to_owned(),
                        serde_json::Value::String("minecraft:iron_ingot".to_owned()),
                    ),
                    ("item_count".to_owned(), serde_json::Value::from(2)),
                ]),
            },
        )
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.add_probe(
        "minecart_count",
        Probe::EntityContainerCount { id: EntityId(1) },
    );

    let first = simulation.step().await.unwrap();
    assert_eq!(first.probes[0].value, ProbeValue::Integer(1));
    assert_eq!(
        simulation.world().entity(EntityId(2)).unwrap().fields["item_count"],
        1
    );
    let second = simulation.step().await.unwrap();
    assert_eq!(second.probes[0].value, ProbeValue::Integer(2));
    assert!(simulation.world().entity(EntityId(2)).is_none());
}

#[tokio::test]
async fn item_pickup_delay_stops_at_zero_and_preserves_never_pickup() {
    let registry = Java26Registry::new();
    let mut world = SparseWorld::new(registry.air_state());
    world
        .spawn_entity_with_id(
            EntityId(1),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [0.5, 0.0, 0.5],
                fields: BTreeMap::from([
                    ("pickup_delay".to_owned(), serde_json::Value::from(0)),
                    ("age".to_owned(), serde_json::Value::from(0)),
                ]),
            },
        )
        .unwrap();
    world
        .spawn_entity_with_id(
            EntityId(2),
            EntityData {
                kind: "minecraft:item".to_owned(),
                position: [2.5, 0.0, 0.5],
                fields: BTreeMap::from([
                    ("pickup_delay".to_owned(), serde_json::Value::from(32_767)),
                    ("age".to_owned(), serde_json::Value::from(0)),
                ]),
            },
        )
        .unwrap();
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();

    simulation.step().await.unwrap();

    assert_eq!(
        simulation.world().entity(EntityId(1)).unwrap().fields["pickup_delay"],
        0
    );
    assert_eq!(
        simulation.world().entity(EntityId(2)).unwrap().fields["pickup_delay"],
        32_767
    );
}
