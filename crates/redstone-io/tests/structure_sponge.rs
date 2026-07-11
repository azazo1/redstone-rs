mod common;

use std::collections::HashMap;
use std::io::Write;

use common::TestResolver;
use fastnbt::{ByteArray, IntArray, Value};
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{Mirror, Rotation, StructureFormat, StructureLoader, StructureTransform};

#[test]
fn detects_schem_extension() {
    assert_eq!(
        StructureLoader::detect(std::path::Path::new("machine.schem")).unwrap(),
        StructureFormat::SpongeSchematic
    );
}

#[test]
fn loads_v1_with_legacy_data_version() {
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(1)),
        ("Width".to_owned(), Value::Short(1)),
        ("Height".to_owned(), Value::Short(1)),
        ("Length".to_owned(), Value::Short(1)),
        ("PaletteMax".to_owned(), Value::Int(1)),
        (
            "Palette".to_owned(),
            Value::Compound(HashMap::from([(
                "minecraft:stone".to_owned(),
                Value::Int(0),
            )])),
        ),
        (
            "BlockData".to_owned(),
            Value::ByteArray(ByteArray::new(vec![0])),
        ),
    ]);
    let path = std::env::temp_dir().join(format!("redstone-sponge-v1-{}.schem", std::process::id()));
    std::fs::write(&path, fastnbt::to_bytes(&root).unwrap()).unwrap();
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

    assert_eq!(loaded.data_version, Some(1631));
    assert_ne!(loaded.world.get_block(BlockPos::ZERO), BlockStateId(0));
}

#[test]
fn loads_gzip_v2_offsets_block_entities_and_entities() {
    let metadata = HashMap::from([
        ("WEOffsetX".to_owned(), Value::Int(-1)),
        ("WEOffsetY".to_owned(), Value::Int(2)),
        ("WEOffsetZ".to_owned(), Value::Int(3)),
    ]);
    let block_entity = HashMap::from([
        (
            "Id".to_owned(),
            Value::String("minecraft:hopper".to_owned()),
        ),
        (
            "Pos".to_owned(),
            Value::IntArray(IntArray::new(vec![0, 0, 0])),
        ),
        ("TransferCooldown".to_owned(), Value::Int(7)),
    ]);
    let entity = HashMap::from([
        (
            "Id".to_owned(),
            Value::String("minecraft:item_frame".to_owned()),
        ),
        (
            "Pos".to_owned(),
            Value::List(vec![
                Value::Double(100.5),
                Value::Double(64.0),
                Value::Double(200.5),
            ]),
        ),
        ("Facing".to_owned(), Value::Byte(2)),
    ]);
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(2)),
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("Width".to_owned(), Value::Short(2)),
        ("Height".to_owned(), Value::Short(1)),
        ("Length".to_owned(), Value::Short(1)),
        (
            "Offset".to_owned(),
            Value::IntArray(IntArray::new(vec![100, 64, 200])),
        ),
        ("Metadata".to_owned(), Value::Compound(metadata)),
        ("PaletteMax".to_owned(), Value::Int(2)),
        (
            "Palette".to_owned(),
            Value::Compound(HashMap::from([
                ("minecraft:air".to_owned(), Value::Int(0)),
                (
                    "minecraft:repeater[facing=north]".to_owned(),
                    Value::Int(1),
                ),
            ])),
        ),
        (
            "BlockData".to_owned(),
            Value::ByteArray(ByteArray::new(vec![1, 0])),
        ),
        (
            "BlockEntities".to_owned(),
            Value::List(vec![Value::Compound(block_entity)]),
        ),
        (
            "Entities".to_owned(),
            Value::List(vec![Value::Compound(entity)]),
        ),
    ]);
    let path = std::env::temp_dir().join(format!("redstone-sponge-v2-{}.schem", std::process::id()));
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&fastnbt::to_bytes(&root).unwrap())
        .unwrap();
    std::fs::write(&path, encoder.finish().unwrap()).unwrap();
    let transform = StructureTransform {
        origin: BlockPos::new(10, 20, 30),
        rotation: Rotation::Clockwise90,
        mirror: Mirror::None,
    };
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load_transformed(&path, transform, &mut resolver).unwrap();

    let block_pos = BlockPos::new(7, 22, 29);
    assert_eq!(loaded.format, "sponge_schematic");
    assert_eq!(loaded.data_version, Some(4790));
    assert_ne!(loaded.world.get_block(block_pos), BlockStateId(0));
    assert_eq!(loaded.region_min, BlockPos::new(7, 22, 29));
    assert_eq!(loaded.region_max, BlockPos::new(7, 22, 30));
    assert_eq!(
        loaded.world.block_entity(block_pos).unwrap().fields["cooldown"],
        7
    );
    assert!(resolver.states.keys().any(|state| {
        state.contains("minecraft:repeater")
            && state.contains("delay\": \"1")
            && state.contains("facing\": \"east")
            && state.contains("locked\": \"false")
            && state.contains("powered\": \"false")
    }));
    let entity = loaded.world.entities().next().unwrap().1;
    assert_eq!(entity.position, [6.5, 22.0, 29.5]);
    assert_eq!(entity.fields["facing"], "east");
}

#[test]
fn loads_v3_nested_blocks_and_relative_offset() {
    let block_entity = HashMap::from([
        (
            "Id".to_owned(),
            Value::String("minecraft:hopper".to_owned()),
        ),
        (
            "Pos".to_owned(),
            Value::IntArray(IntArray::new(vec![0, 0, 0])),
        ),
        (
            "Data".to_owned(),
            Value::Compound(HashMap::from([(
                "TransferCooldown".to_owned(),
                Value::Int(9),
            )])),
        ),
    ]);
    let blocks = HashMap::from([
        (
            "Palette".to_owned(),
            Value::Compound(HashMap::from([(
                "minecraft:hopper[facing=down,enabled=true]".to_owned(),
                Value::Int(0),
            )])),
        ),
        (
            "Data".to_owned(),
            Value::ByteArray(ByteArray::new(vec![0])),
        ),
        (
            "BlockEntities".to_owned(),
            Value::List(vec![Value::Compound(block_entity)]),
        ),
    ]);
    let schematic = HashMap::from([
        ("Version".to_owned(), Value::Int(3)),
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("Width".to_owned(), Value::Short(1)),
        ("Height".to_owned(), Value::Short(1)),
        ("Length".to_owned(), Value::Short(1)),
        (
            "Offset".to_owned(),
            Value::IntArray(IntArray::new(vec![2, -1, 4])),
        ),
        ("Blocks".to_owned(), Value::Compound(blocks)),
    ]);
    let root = HashMap::from([("Schematic".to_owned(), Value::Compound(schematic))]);
    let path = std::env::temp_dir().join(format!("redstone-sponge-v3-{}.schem", std::process::id()));
    std::fs::write(&path, fastnbt::to_bytes(&root).unwrap()).unwrap();
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

    let block_pos = BlockPos::new(2, -1, 4);
    assert_ne!(loaded.world.get_block(block_pos), BlockStateId(0));
    assert_eq!(loaded.region_min, block_pos);
    assert_eq!(loaded.region_max, block_pos);
    assert_eq!(
        loaded.world.block_entity(block_pos).unwrap().fields["cooldown"],
        9
    );
}
