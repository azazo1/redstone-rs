use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{
    Action, BlockPos, EntityData, EntityId, GameTick, Simulation, SimulationConfig,
};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{
    Java26Registry, Java26Rules, StateResolveError, StateResolver,
};

const CHEST_POS: BlockPos = BlockPos::new(0, 3, 3);
const TRIPWIRE_POS: BlockPos = BlockPos::new(2, 3, 2);
const PRESSURE_PLATE_POS: BlockPos = BlockPos::new(6, 4, 3);
const OBSERVER_LEVER_POS: BlockPos = BlockPos::new(8, 3, 0);
const OBSERVER_LAMP_POS: BlockPos = BlockPos::new(8, 4, 4);

struct RegistryResolver(Java26Registry);

impl StructureStateResolver for RegistryResolver {
    type Error = StateResolveError;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<redstone_core::BlockStateId, Self::Error> {
        self.0.resolve_state(name, properties)
    }

    fn air_state(&self) -> redstone_core::BlockStateId {
        self.0.air_state()
    }
}

#[tokio::test]
async fn gold_marked_lamps_turn_on_and_unmarked_lamps_stay_off() {
    let (simulation, lamps, marked_lamps) = load_charging_tests(true).await;

    assert_eq!(lamps.len(), 26);
    assert_eq!(marked_lamps.len(), 18);
    for lamp_pos in lamps {
        let expected = marked_lamps.contains(&lamp_pos) && lamp_pos != OBSERVER_LAMP_POS;
        assert_eq!(
            lamp_is_lit(&simulation, lamp_pos),
            expected,
            "红石灯标记结果错误: {lamp_pos:?}"
        );
    }
}

#[tokio::test]
async fn observer_lamp_turns_on_only_during_the_observer_pulse() {
    let (mut simulation, _, _) = load_charging_tests(false).await;
    assert!(!lamp_is_lit(&simulation, OBSERVER_LAMP_POS));

    simulation
        .step_with_actions(&[Action::PullLever {
            pos: OBSERVER_LEVER_POS,
        }])
        .await
        .unwrap();

    let mut turned_on = lamp_is_lit(&simulation, OBSERVER_LAMP_POS);
    for _ in 0..12 {
        simulation.step().await.unwrap();
        turned_on |= lamp_is_lit(&simulation, OBSERVER_LAMP_POS);
    }
    assert!(turned_on, "侦测器脉冲没有点亮红石灯");

    let settled_tick = GameTick(simulation.current_tick().0 + 20);
    simulation.run_until(settled_tick).await.unwrap();
    assert!(!lamp_is_lit(&simulation, OBSERVER_LAMP_POS));
}

async fn load_charging_tests(
    activate_sources: bool,
) -> (Simulation<Java26Rules>, Vec<BlockPos>, Vec<BlockPos>) {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/charging-tests.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let mut loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();

    let mut lamps = loaded
        .world
        .iter_blocks()
        .filter_map(|(pos, state_id)| {
            resolver
                .0
                .state(state_id)
                .is_some_and(|state| state.name == "minecraft:redstone_lamp")
                .then_some(pos)
        })
        .collect::<Vec<_>>();
    lamps.sort_unstable();
    let marked_lamps = lamps
        .iter()
        .copied()
        .filter(|pos| {
            resolver
                .0
                .state(loaded.world.get_block(pos.offset(0, 1, 0)))
                .is_some_and(|state| state.name == "minecraft:gold_block")
        })
        .collect::<Vec<_>>();

    if activate_sources {
        loaded
            .world
            .block_entity_mut(CHEST_POS)
            .unwrap()
            .fields
            .insert("open_count".to_owned(), serde_json::Value::from(1));
        for (id, pos) in [
            (EntityId(100), TRIPWIRE_POS),
            (EntityId(101), PRESSURE_PLATE_POS),
        ] {
            loaded
                .world
                .spawn_entity_with_id(
                    id,
                    EntityData {
                        kind: "minecraft:generic_collision".to_owned(),
                        position: [pos.x as f64 + 0.5, pos.y as f64 + 0.1, pos.z as f64 + 0.5],
                        fields: BTreeMap::new(),
                    },
                )
                .unwrap();
        }
    }

    let rules = Java26Rules::new(resolver.0);
    let mut simulation = Simulation::load(rules, loaded.world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();
    simulation.run_until(GameTick(20)).await.unwrap();
    (simulation, lamps, marked_lamps)
}

fn lamp_is_lit(simulation: &Simulation<Java26Rules>, pos: BlockPos) -> bool {
    let state_id = simulation.world().get_block(pos);
    simulation
        .rules()
        .registry()
        .state(state_id)
        .is_some_and(|state| state.property("lit") == Some("true"))
}
