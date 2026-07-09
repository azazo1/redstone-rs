use std::collections::{BTreeMap, HashMap};
use std::io::Write;

use fastnbt::{LongArray, Value};
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{
    Mirror, Rotation, StructureLoader, StructureStateResolver, StructureTransform,
};

#[derive(Default)]
struct TestResolver {
    states: BTreeMap<String, BlockStateId>,
}

impl StructureStateResolver for TestResolver {
    type Error = std::io::Error;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, Self::Error> {
        let key = format!("{name}{properties:?}");
        let next = BlockStateId(self.states.len() as u32 + 1);
        Ok(*self.states.entry(key).or_insert(next))
    }

    fn air_state(&self) -> BlockStateId {
        BlockStateId(0)
    }
}

#[test]
fn vanilla_structure_round_trip_loads_blocks_block_entities_and_entities() {
    let mut root = HashMap::new();
    root.insert("DataVersion".to_owned(), Value::Int(4790));
    root.insert(
        "size".to_owned(),
        Value::List(vec![Value::Int(2), Value::Int(1), Value::Int(1)]),
    );
    root.insert(
        "palette".to_owned(),
        Value::List(vec![Value::Compound(HashMap::from([(
            "Name".to_owned(),
            Value::String("minecraft:observer".to_owned()),
        ), (
            "Properties".to_owned(),
            Value::Compound(HashMap::from([
                ("facing".to_owned(), Value::String("north".to_owned())),
                ("powered".to_owned(), Value::String("false".to_owned())),
            ])),
        )]))]),
    );
    root.insert(
        "blocks".to_owned(),
        Value::List(vec![Value::Compound(HashMap::from([
            (
                "pos".to_owned(),
                Value::List(vec![Value::Int(1), Value::Int(0), Value::Int(0)]),
            ),
            ("state".to_owned(), Value::Int(0)),
            (
                "nbt".to_owned(),
                Value::Compound(HashMap::from([
                    ("id".to_owned(), Value::String("minecraft:hopper".to_owned())),
                    ("TransferCooldown".to_owned(), Value::Int(6)),
                    (
                        "Items".to_owned(),
                        Value::List(vec![Value::Compound(HashMap::from([
                            ("Slot".to_owned(), Value::Byte(2)),
                            ("id".to_owned(), Value::String("minecraft:redstone".to_owned())),
                            ("count".to_owned(), Value::Int(12)),
                        ]))]),
                    ),
                ])),
            ),
        ]))]),
    );
    root.insert(
        "entities".to_owned(),
        Value::List(vec![Value::Compound(HashMap::from([
            (
                "pos".to_owned(),
                Value::List(vec![Value::Double(0.5), Value::Double(0.0), Value::Double(0.5)]),
            ),
            (
                "nbt".to_owned(),
                Value::Compound(HashMap::from([(
                    "id".to_owned(),
                    Value::String("minecraft:item_frame".to_owned()),
                )])),
            ),
        ]))]),
    );

    let path = std::env::temp_dir().join(format!(
        "redstone-structure-{}.nbt",
        std::process::id()
    ));
    std::fs::write(&path, fastnbt::to_bytes(&root).unwrap()).unwrap();
    let transform = StructureTransform {
        origin: BlockPos::new(10, 20, 30),
        rotation: Rotation::Clockwise90,
        mirror: Mirror::None,
    };
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load_transformed(&path, transform, &mut resolver).unwrap();

    let block_pos = BlockPos::new(10, 20, 31);
    assert_ne!(loaded.world.get_block(block_pos), BlockStateId(0));
    let block_entity = loaded.world.block_entity(block_pos).unwrap();
    assert_eq!(block_entity.fields["item_count"], serde_json::Value::from(12));
    assert_eq!(block_entity.fields["item_id"], "minecraft:redstone");
    assert_eq!(block_entity.fields["slot_count"], serde_json::Value::from(5));
    assert_eq!(block_entity.fields["capacity"], serde_json::Value::from(320));
    assert_eq!(block_entity.fields["cooldown"], serde_json::Value::from(6));
    let entity = loaded.world.entities().next().unwrap().1;
    assert_eq!(entity.kind, "minecraft:item_frame");
    assert_eq!(entity.position, [9.5, 20.0, 30.5]);
}

#[test]
fn gzip_litematic_loads_negative_regions_from_their_minimum_corner() {
    let mut region = HashMap::new();
    region.insert(
        "Position".to_owned(),
        xyz_compound(4, 0, 0),
    );
    region.insert("Size".to_owned(), xyz_compound(-2, 1, 1));
    region.insert(
        "BlockStatePalette".to_owned(),
        Value::List(vec![
            block_state("minecraft:air", &[]),
            block_state(
                "minecraft:repeater",
                &[
                    ("facing", "north"),
                    ("delay", "1"),
                    ("locked", "false"),
                    ("powered", "false"),
                ],
            ),
        ]),
    );
    region.insert(
        "BlockStates".to_owned(),
        Value::LongArray(LongArray::new(vec![0b0101])),
    );
    region.insert("TileEntities".to_owned(), Value::List(Vec::new()));
    region.insert(
        "Entities".to_owned(),
        Value::List(vec![Value::Compound(HashMap::from([
            ("id".to_owned(), Value::String("minecraft:item".to_owned())),
            (
                "Pos".to_owned(),
                Value::List(vec![Value::Double(0.25), Value::Double(0.0), Value::Double(0.5)]),
            ),
        ]))]),
    );
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(7)),
        ("MinecraftDataVersion".to_owned(), Value::Int(4790)),
        (
            "Regions".to_owned(),
            Value::Compound(HashMap::from([("main".to_owned(), Value::Compound(region))])),
        ),
    ]);

    let path = std::env::temp_dir().join(format!(
        "redstone-litematic-{}.litematic",
        std::process::id()
    ));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&fastnbt::to_bytes(&root).unwrap()).unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

    assert_ne!(loaded.world.get_block(BlockPos::new(3, 0, 0)), BlockStateId(0));
    assert_ne!(loaded.world.get_block(BlockPos::new(4, 0, 0)), BlockStateId(0));
    assert_eq!(loaded.min, BlockPos::new(3, 0, 0));
    assert_eq!(loaded.max, BlockPos::new(4, 0, 0));
    let entity = loaded.world.entities().next().unwrap().1;
    assert_eq!(entity.position, [4.25, 0.0, 0.5]);
}

fn xyz_compound(x: i32, y: i32, z: i32) -> Value {
    Value::Compound(HashMap::from([
        ("x".to_owned(), Value::Int(x)),
        ("y".to_owned(), Value::Int(y)),
        ("z".to_owned(), Value::Int(z)),
    ]))
}

fn block_state(name: &str, properties: &[(&str, &str)]) -> Value {
    let mut state = HashMap::from([(
        "Name".to_owned(),
        Value::String(name.to_owned()),
    )]);
    if !properties.is_empty() {
        state.insert(
            "Properties".to_owned(),
            Value::Compound(
                properties
                    .iter()
                    .map(|(name, value)| {
                        ((*name).to_owned(), Value::String((*value).to_owned()))
                    })
                    .collect(),
            ),
        );
    }
    Value::Compound(state)
}
