use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{
    Action, BlockPos, GameTick, RedstoneMode, Simulation, SimulationConfig,
};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{
    Java26Registry, Java26Rules, StateResolveError, StateResolver,
};

const INPUT_MASKS: [u8; 3] = [0b100, 0b010, 0b001];
const INPUT_ORDER: [u8; 8] = [0, 4, 5, 7, 6, 2, 3, 1];
const OUTPUTS: [BlockPos; 8] = [
    BlockPos::new(9, 0, 14),
    BlockPos::new(9, 0, 12),
    BlockPos::new(9, 0, 10),
    BlockPos::new(9, 0, 8),
    BlockPos::new(9, 0, 6),
    BlockPos::new(9, 0, 4),
    BlockPos::new(9, 0, 2),
    BlockPos::new(9, 0, 0),
];

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
async fn default_wire_mode_decodes_every_three_bit_input() {
    assert_all_inputs(RedstoneMode::Default).await;
}

#[tokio::test]
async fn experimental_wire_mode_decodes_every_three_bit_input() {
    assert_all_inputs(RedstoneMode::Experimental).await;
}

async fn assert_all_inputs(mode: RedstoneMode) {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/mux3-8.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();

    let mut levers = loaded
        .world
        .iter_blocks()
        .filter_map(|(pos, state_id)| {
            resolver
                .0
                .state(state_id)
                .is_some_and(|state| state.name == "minecraft:lever")
                .then_some(pos)
        })
        .collect::<Vec<_>>();
    levers.sort_by_key(|pos| pos.x);
    assert_eq!(levers.len(), INPUT_MASKS.len());

    let rules = Java26Rules::new(resolver.0);
    let mut simulation = Simulation::load(
        rules,
        loaded.world,
        SimulationConfig {
            mode,
            ..SimulationConfig::default()
        },
    )
    .await
    .unwrap();
    simulation.initialize().await.unwrap();
    simulation.run_until(GameTick(20)).await.unwrap();

    let mut previous = 0;
    for input in INPUT_ORDER {
        let changed = previous ^ input;
        if changed != 0 {
            let lever = INPUT_MASKS
                .iter()
                .position(|mask| changed == *mask)
                .unwrap();
            simulation
                .step_with_actions(&[Action::PullLever {
                    pos: levers[lever],
                }])
                .await
                .unwrap();
            let settled_tick = GameTick(simulation.current_tick().0 + 20);
            simulation.run_until(settled_tick).await.unwrap();
        }
        assert_output(&simulation, input, mode);
        previous = input;
    }
}

fn assert_output(simulation: &Simulation<Java26Rules>, input: u8, mode: RedstoneMode) {
    let active = OUTPUTS.map(|pos| {
        let state_id = simulation.world().get_block(pos);
        simulation
            .rules()
            .registry()
            .state(state_id)
            .is_some_and(|state| state.property("lit") == Some("true"))
    });
    let expected = std::array::from_fn(|output| output == input as usize);
    assert_eq!(active, expected, "模式 {mode:?}, 输入 {input:03b}");
}
