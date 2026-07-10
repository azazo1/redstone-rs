use std::collections::BTreeMap;
use std::path::PathBuf;

use redstone_core::{Action, BlockPos, GameTick, Simulation, SimulationConfig};
use redstone_io::{StructureLoader, StructureStateResolver};
use redstone_java_26::{
    Java26Registry, Java26Rules, StateResolveError, StateResolver,
};

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
async fn each_note_block_selects_only_the_lamp_above_it_with_initialized_droppers() {
    let schematic = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/schematics/one-chooser.litematic");
    let mut resolver = RegistryResolver(Java26Registry::new());
    let mut loaded = StructureLoader::load(&schematic, BlockPos::ZERO, &mut resolver).unwrap();

    let mut note_blocks = Vec::new();
    let mut lamps = Vec::new();
    for (pos, state_id) in loaded.world.iter_blocks() {
        let state = resolver.0.state(state_id).unwrap();
        match state.name.as_str() {
            "minecraft:note_block" => note_blocks.push(pos),
            "minecraft:redstone_lamp" => lamps.push(pos),
            _ => {}
        }
    }
    note_blocks.sort();
    lamps.sort();
    assert_eq!(note_blocks.len(), 8);
    assert_eq!(lamps.len(), 8);

    let lower_droppers = loaded
        .world
        .block_entities()
        .filter(|(pos, data)| pos.y == 1 && data.kind == "minecraft:dropper")
        .map(|(pos, _)| *pos)
        .collect::<Vec<_>>();
    assert_eq!(lower_droppers.len(), 8);
    for pos in lower_droppers {
        let data = loaded.world.block_entity_mut(pos).unwrap();
        data.fields.insert(
            "inventory".to_owned(),
            serde_json::json!([{
                "slot": 0,
                "item_id": "minecraft:stone",
                "count": 1,
            }]),
        );
        data.fields
            .insert("item_count".to_owned(), serde_json::Value::from(1));
    }

    let rules = Java26Rules::new(resolver.0);
    let mut simulation = Simulation::load(rules, loaded.world, SimulationConfig::default())
        .await
        .unwrap();
    simulation.initialize().await.unwrap();
    simulation.run_until(GameTick(12)).await.unwrap();

    for note_pos in note_blocks {
        simulation
            .step_with_actions(&[Action::UseBlock { pos: note_pos }])
            .await
            .unwrap();
        let settled_tick = GameTick(simulation.current_tick().0 + 20);
        simulation.run_until(settled_tick).await.unwrap();

        let lit = lamps
            .iter()
            .copied()
            .filter(|lamp_pos| {
                let state_id = simulation.world().get_block(*lamp_pos);
                simulation
                    .rules()
                    .registry()
                    .state(state_id)
                    .is_some_and(|state| state.property("lit") == Some("true"))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            lit.len(),
            1,
            "note block at {note_pos:?} selected {lit:?}"
        );
        assert_eq!(
            lit[0].x,
            note_pos.x,
            "note={note_pos:?}, lit={lit:?}"
        );
        assert_eq!(
            lit[0].z,
            note_pos.z,
            "note={note_pos:?}, lit={lit:?}"
        );
        assert!(lit[0].y > note_pos.y);
    }
}
