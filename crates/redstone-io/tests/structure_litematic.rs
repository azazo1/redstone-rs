mod common;

use std::collections::HashMap;
use std::io::Write;

use common::{TestResolver, block_state};
use fastnbt::{LongArray, Value};
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::StructureLoader;

#[test]
fn loads_gzip_negative_regions_from_their_minimum_corner() {
    let mut region = HashMap::new();
    region.insert("Position".to_owned(), xyz_compound(4, 0, 0));
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
                Value::List(vec![
                    Value::Double(0.25),
                    Value::Double(0.0),
                    Value::Double(0.5),
                ]),
            ),
        ]))]),
    );
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(7)),
        ("MinecraftDataVersion".to_owned(), Value::Int(4790)),
        (
            "Regions".to_owned(),
            Value::Compound(HashMap::from([(
                "main".to_owned(),
                Value::Compound(region),
            )])),
        ),
    ]);

    let path = std::env::temp_dir().join(format!(
        "redstone-litematic-{}.litematic",
        std::process::id()
    ));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&fastnbt::to_bytes(&root).unwrap())
        .unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

    assert_ne!(
        loaded.world.get_block(BlockPos::new(3, 0, 0)),
        BlockStateId(0)
    );
    assert_ne!(
        loaded.world.get_block(BlockPos::new(4, 0, 0)),
        BlockStateId(0)
    );
    assert_eq!(loaded.min, BlockPos::new(3, 0, 0));
    assert_eq!(loaded.max, BlockPos::new(4, 0, 0));
    assert_eq!(loaded.region_min, BlockPos::new(3, 0, 0));
    assert_eq!(loaded.region_max, BlockPos::new(4, 0, 0));
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
