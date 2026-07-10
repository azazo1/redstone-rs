use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{Action, BlockPos, GameTick, RedstoneMode, Simulation, SimulationConfig};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{Java26Registry, Java26Rules, StateResolveError, StateResolver};

const SEGMENTS: [[BlockPos; 3]; 7] = [
    [
        BlockPos::new(4, 11, 23),
        BlockPos::new(5, 11, 23),
        BlockPos::new(6, 11, 23),
    ],
    [
        BlockPos::new(7, 8, 23),
        BlockPos::new(7, 9, 23),
        BlockPos::new(7, 10, 23),
    ],
    [
        BlockPos::new(7, 4, 23),
        BlockPos::new(7, 5, 23),
        BlockPos::new(7, 6, 23),
    ],
    [
        BlockPos::new(4, 3, 23),
        BlockPos::new(5, 3, 23),
        BlockPos::new(6, 3, 23),
    ],
    [
        BlockPos::new(3, 4, 23),
        BlockPos::new(3, 5, 23),
        BlockPos::new(3, 6, 23),
    ],
    [
        BlockPos::new(3, 8, 23),
        BlockPos::new(3, 9, 23),
        BlockPos::new(3, 10, 23),
    ],
    [
        BlockPos::new(4, 7, 23),
        BlockPos::new(5, 7, 23),
        BlockPos::new(6, 7, 23),
    ],
];

const DIGIT_SEGMENTS: [[bool; 7]; 10] = [
    [true, true, true, true, true, true, false],
    [false, true, true, false, false, false, false],
    [true, true, false, true, true, false, true],
    [true, true, true, true, false, false, true],
    [false, true, true, false, false, true, true],
    [true, false, true, true, false, true, true],
    [true, false, true, true, true, true, true],
    [true, true, true, false, false, false, false],
    [true, true, true, true, true, true, true],
    [true, true, true, true, false, true, true],
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

#[test]
fn default_wire_mode_displays_each_decimal_input_with_seven_segments() {
    assert_all_digits(RedstoneMode::Default);
}

#[test]
fn experimental_wire_mode_displays_each_decimal_input_with_seven_segments() {
    assert_all_digits(RedstoneMode::Experimental);
}

fn assert_all_digits(mode: RedstoneMode) {
    let schematic =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/schematics/seg7.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();

    let mut levers = loaded
        .world
        .iter_blocks()
        .filter_map(|(pos, state_id)| {
            resolver
                .0
                .state(state_id)
                .is_some_and(|state| state.name.as_ref() == "minecraft:lever")
                .then_some(pos)
        })
        .collect::<Vec<_>>();
    levers.sort_by_key(|pos| pos.z);
    assert_eq!(levers.len(), 10);

    let lamp_count = loaded
        .world
        .iter_blocks()
        .filter(|(_, state_id)| {
            resolver
                .0
                .state(*state_id)
                .is_some_and(|state| state.name.as_ref() == "minecraft:redstone_lamp")
        })
        .count();
    assert_eq!(lamp_count, 21);

    let rules = Java26Rules::new(resolver.0);
    let config = SimulationConfig {
        mode,
        ..SimulationConfig::default()
    };
    let mut simulation = Simulation::load(rules, loaded.world, config).unwrap();
    simulation.initialize().unwrap();
    simulation.run_until(GameTick(20)).unwrap();

    assert_digit(&simulation, 9, mode);
    for digit in (0..9).rev() {
        let previous = levers[9 - (digit + 1)];
        let selected = levers[9 - digit];
        simulation
            .step_with_actions(&[Action::PullLever { pos: previous }])
            .unwrap();
        let settled_tick = GameTick(simulation.current_tick().0 + 20);
        simulation.run_until(settled_tick).unwrap();
        simulation
            .step_with_actions(&[Action::PullLever { pos: selected }])
            .unwrap();
        let settled_tick = GameTick(simulation.current_tick().0 + 20);
        simulation.run_until(settled_tick).unwrap();
        assert_digit(&simulation, digit, mode);
    }
}

fn assert_digit(simulation: &Simulation<Java26Rules>, digit: usize, mode: RedstoneMode) {
    const SEGMENT_NAMES: [char; 7] = ['A', 'B', 'C', 'D', 'E', 'F', 'G'];

    let actual = SEGMENTS.map(|positions| {
        let lamps = positions.map(|pos| {
            let state_id = simulation.world().get_block(pos);
            let state = simulation.rules().registry().state(state_id).unwrap();
            state.property("lit") == Some("true")
        });
        assert!(
            lamps.iter().all(|lit| *lit == lamps[0]),
            "mode {mode:?}, digit {digit}, segment lamps are inconsistent: {positions:?} => {lamps:?}"
        );
        lamps[0]
    });
    assert_eq!(
        actual, DIGIT_SEGMENTS[digit],
        "mode {mode:?}, digit {digit}, segments {SEGMENT_NAMES:?}"
    );
}
