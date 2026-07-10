use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{Action, BlockPos, GameTick, Simulation, SimulationConfig};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{Java26Registry, Java26Rules, StateResolveError, StateResolver};

const BUTTON_POS: BlockPos = BlockPos::new(3, 1, 1);
const RETRACTED_WOOL_POS: BlockPos = BlockPos::new(5, 0, 5);
const EXTENDED_WOOL_POS: BlockPos = BlockPos::new(8, 0, 5);

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
async fn button_moves_wool_three_blocks_out_and_back() {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/tripple-piston-extender.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();
    let wool = resolver
        .0
        .resolve_state("minecraft:orange_wool", &BTreeMap::new())
        .unwrap();

    let rules = Java26Rules::new(resolver.0);
    let mut simulation = Simulation::load(rules, loaded.world, SimulationConfig::default())
        .await
        .unwrap();

    assert_settled_wool_at(&simulation, wool, RETRACTED_WOOL_POS);

    press_button_and_settle(&mut simulation).await;
    assert_settled_wool_at(&simulation, wool, EXTENDED_WOOL_POS);

    press_button_and_settle(&mut simulation).await;
    assert_settled_wool_at(&simulation, wool, RETRACTED_WOOL_POS);
}

async fn press_button_and_settle(simulation: &mut Simulation<Java26Rules>) {
    simulation
        .step_with_actions(&[Action::PressButton { pos: BUTTON_POS }])
        .await
        .unwrap();
    let settled_tick = GameTick(simulation.current_tick().0 + 50);
    simulation.run_until(settled_tick).await.unwrap();
}

fn assert_settled_wool_at(
    simulation: &Simulation<Java26Rules>,
    wool: redstone_core::BlockStateId,
    expected: BlockPos,
) {
    let actual = wool_positions(simulation, wool);
    assert_eq!(actual, [expected]);
    assert!(
        simulation
            .world()
            .block_entities()
            .all(|(_, data)| data.kind != "minecraft:moving_piston")
    );
}

fn wool_positions(
    simulation: &Simulation<Java26Rules>,
    wool: redstone_core::BlockStateId,
) -> Vec<BlockPos> {
    simulation
        .world()
        .iter_blocks()
        .filter_map(|(pos, state)| (pos.y == 0 && pos.z == 5 && state == wool).then_some(pos))
        .collect()
}
