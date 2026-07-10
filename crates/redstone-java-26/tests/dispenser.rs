use std::collections::BTreeMap;

use redstone_core::{
    BlockEntityData, BlockPos, BlockStateId, GameTick, Simulation, SimulationConfig, SparseWorld,
    TraceKind,
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

fn dispenser_inventory(item: &str) -> BlockEntityData {
    BlockEntityData {
        kind: "minecraft:dispenser".to_owned(),
        fields: BTreeMap::from([
            (
                "inventory".to_owned(),
                serde_json::json!([{
                    "slot": 0,
                    "item_id": item,
                    "count": 1,
                }]),
            ),
            ("item_count".to_owned(), serde_json::Value::from(1)),
            ("slot_count".to_owned(), serde_json::Value::from(9)),
            ("capacity".to_owned(), serde_json::Value::from(576)),
        ]),
    }
}

async fn dispenser_world(item: &str, rail_in_front: bool) -> Simulation<Java26Rules> {
    let mut registry = Java26Registry::new();
    let dispenser = state(
        &mut registry,
        "minecraft:dispenser",
        &[("facing", "east"), ("triggered", "false")],
    );
    let source = state(&mut registry, "minecraft:redstone_block", &[]);
    let rail = rail_in_front.then(|| {
        state(
            &mut registry,
            "minecraft:rail",
            &[("shape", "east_west"), ("waterlogged", "false")],
        )
    });
    let mut world = SparseWorld::new(registry.air_state());
    world.set_block(BlockPos::ZERO, dispenser).unwrap();
    world.set_block(BlockPos::new(0, 1, 0), source).unwrap();
    if let Some(rail) = rail {
        world.set_block(BlockPos::new(1, 0, 0), rail).unwrap();
    }
    world.set_block_entity(BlockPos::ZERO, dispenser_inventory(item));
    let rules = Java26Rules::new(registry);
    let mut simulation = Simulation::load(rules, world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();
    simulation
}

#[tokio::test]
async fn projectile_entity_records_the_original_dispensed_item() {
    let mut simulation = dispenser_world("minecraft:tipped_arrow", false).await;

    simulation.run_until(GameTick(5)).await.unwrap();

    let entity = simulation.world().entities().next().unwrap().1;
    assert_eq!(entity.kind, "minecraft:arrow");
    assert_eq!(entity.fields["source_item"], "minecraft:tipped_arrow");
    assert_eq!(entity.fields["facing"], "east");
    assert_eq!(entity.fields["velocity"], serde_json::json!([1, 0, 0]));
}

#[tokio::test]
async fn minecart_spawns_on_a_rail_and_falls_back_to_an_item_without_one() {
    let mut on_rail = dispenser_world("minecraft:hopper_minecart", true).await;

    on_rail.run_until(GameTick(5)).await.unwrap();

    let minecart = on_rail.world().entities().next().unwrap().1;
    assert_eq!(minecart.kind, "minecraft:hopper_minecart");
    assert_eq!(minecart.position, [1.5, 0.1, 0.5]);

    let mut fallback = dispenser_world("minecraft:hopper_minecart", false).await;
    fallback.run_until(GameTick(5)).await.unwrap();
    let item = fallback.world().entities().next().unwrap().1;
    assert_eq!(item.kind, "minecraft:item");
    assert_eq!(item.fields["item_id"], "minecraft:hopper_minecart");
}

#[tokio::test]
async fn dispensed_tnt_counts_down_and_records_an_unsupported_explosion() {
    let mut simulation = dispenser_world("minecraft:tnt", false).await;

    simulation.run_until(GameTick(5)).await.unwrap();

    let tnt = simulation.world().entities().next().unwrap().1;
    assert_eq!(tnt.kind, "minecraft:tnt");
    assert_eq!(tnt.fields["fuse"], 78);
    assert_eq!(tnt.fields["ignited_by"], "dispenser");

    simulation.run_until(GameTick(84)).await.unwrap();
    assert!(
        simulation
            .world()
            .entities()
            .all(|(_, entity)| entity.kind != "minecraft:tnt")
    );
    assert!(simulation.trace().events().iter().any(|event| {
        matches!(
            &event.kind,
            TraceKind::UnsupportedTrigger { behavior, .. } if behavior == "tnt_explosion"
        )
    }));
}
