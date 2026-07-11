mod common;

use std::collections::HashMap;

use common::{TestResolver, block_state};
use fastnbt::{IntArray, Value};
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{Mirror, Rotation, StructureLoader, StructureTransform};

#[test]
fn loads_blocks_block_entities_and_entities() {
    let mut root = HashMap::new();
    root.insert("DataVersion".to_owned(), Value::Int(4790));
    root.insert(
        "size".to_owned(),
        Value::List(vec![Value::Int(2), Value::Int(1), Value::Int(1)]),
    );
    root.insert(
        "palette".to_owned(),
        Value::List(vec![block_state(
            "minecraft:observer",
            &[("facing", "north"), ("powered", "false")],
        )]),
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
                    (
                        "id".to_owned(),
                        Value::String("minecraft:hopper".to_owned()),
                    ),
                    ("TransferCooldown".to_owned(), Value::Int(6)),
                    (
                        "Items".to_owned(),
                        Value::List(vec![Value::Compound(HashMap::from([
                            ("Slot".to_owned(), Value::Byte(2)),
                            (
                                "id".to_owned(),
                                Value::String("minecraft:redstone".to_owned()),
                            ),
                            ("count".to_owned(), Value::Int(12)),
                            (
                                "components".to_owned(),
                                Value::Compound(HashMap::from([(
                                    "minecraft:custom_name".to_owned(),
                                    Value::String("{\"text\":\"Signal\"}".to_owned()),
                                )])),
                            ),
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
                Value::List(vec![
                    Value::Double(0.5),
                    Value::Double(0.0),
                    Value::Double(0.5),
                ]),
            ),
            (
                "nbt".to_owned(),
                Value::Compound(HashMap::from([
                    (
                        "id".to_owned(),
                        Value::String("minecraft:item_frame".to_owned()),
                    ),
                    ("Facing".to_owned(), Value::Byte(2)),
                    ("ItemRotation".to_owned(), Value::Byte(5)),
                    (
                        "Item".to_owned(),
                        Value::Compound(HashMap::from([
                            ("id".to_owned(), Value::String("minecraft:map".to_owned())),
                            ("count".to_owned(), Value::Int(1)),
                        ])),
                    ),
                ])),
            ),
        ]))]),
    );

    let path = std::env::temp_dir().join(format!("redstone-structure-{}.nbt", std::process::id()));
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
    assert_eq!(loaded.region_min, BlockPos::new(10, 20, 30));
    assert_eq!(loaded.region_max, BlockPos::new(10, 20, 31));
    let block_entity = loaded.world.block_entity(block_pos).unwrap();
    assert_eq!(block_entity.fields["item_count"], 12);
    assert_eq!(block_entity.fields["item_id"], "minecraft:redstone");
    assert_eq!(block_entity.fields["slot_count"], 5);
    assert_eq!(block_entity.fields["capacity"], 320);
    assert_eq!(block_entity.fields["cooldown"], 6);
    assert_eq!(
        block_entity.fields["inventory"][0]["components"]["minecraft:custom_name"],
        "{\"text\":\"Signal\"}"
    );
    let entity = loaded.world.entities().next().unwrap().1;
    assert_eq!(entity.kind, "minecraft:item_frame");
    assert_eq!(entity.position, [9.5, 20.0, 30.5]);
    assert_eq!(entity.fields["item_id"], "minecraft:map");
    assert_eq!(entity.fields["item_count"], 1);
    assert_eq!(entity.fields["rotation"], 5);
    assert_eq!(entity.fields["facing"], "east");
}

#[test]
fn preserves_crafter_disabled_slots() {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![block_state(
                "minecraft:crafter",
                &[
                    ("crafting", "false"),
                    ("orientation", "north_up"),
                    ("triggered", "false"),
                ],
            )]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                (
                    "pos".to_owned(),
                    Value::List(vec![Value::Int(0), Value::Int(0), Value::Int(0)]),
                ),
                ("state".to_owned(), Value::Int(0)),
                (
                    "nbt".to_owned(),
                    Value::Compound(HashMap::from([
                        (
                            "id".to_owned(),
                            Value::String("minecraft:crafter".to_owned()),
                        ),
                        ("Items".to_owned(), Value::List(Vec::new())),
                        (
                            "disabled_slots".to_owned(),
                            Value::IntArray(IntArray::new(vec![0, 3, 8])),
                        ),
                    ])),
                ),
            ]))]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    let path = std::env::temp_dir().join(format!("redstone-crafter-{}.nbt", std::process::id()));
    std::fs::write(&path, fastnbt::to_bytes(&root).unwrap()).unwrap();
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

    assert_eq!(
        loaded.world.block_entity(BlockPos::ZERO).unwrap().fields["disabled_slots"],
        serde_json::json!([0, 3, 8])
    );
}
