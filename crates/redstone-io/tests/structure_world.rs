mod common;

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use common::TestResolver;
use fastnbt::Value;
use flate2::{Compression, write::GzEncoder};
use redstone_core::{BlockPos, BlockStateId};
use redstone_io::{StructureLoadOptions, StructureLoader, StructureRegion};

#[test]
fn explicit_region_loads_blocks_block_entities_and_entities_from_normal_world() {
    let directory = TestDirectory::new();
    write_world_settings(directory.path(), false);
    let dimension = directory
        .path()
        .join("dimensions/minecraft/overworld");
    write_region(
        &dimension.join("region/r.0.0.mca"),
        0,
        block_chunk(),
    );
    write_region(
        &dimension.join("entities/r.0.0.mca"),
        0,
        entity_chunk(),
    );
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load_with_options(
        directory.path(),
        StructureLoadOptions {
            region: Some(StructureRegion::new(
                BlockPos::new(0, 0, 0),
                BlockPos::new(15, 15, 15),
            )),
            ..StructureLoadOptions::default()
        },
        &mut resolver,
    )
    .unwrap();

    assert_ne!(loaded.world.get_block(BlockPos::ZERO), BlockStateId(0));
    assert_eq!(loaded.world.block_entities().count(), 1);
    assert_eq!(loaded.world.entities().count(), 1);
    assert_eq!(loaded.region_min, BlockPos::ZERO);
    assert_eq!(loaded.region_max, BlockPos::new(15, 15, 15));
}

#[test]
fn normal_world_without_region_is_rejected() {
    let directory = TestDirectory::new();
    write_world_settings(directory.path(), false);
    let mut resolver = TestResolver::default();
    let error = StructureLoader::load(directory.path(), BlockPos::ZERO, &mut resolver).unwrap_err();
    assert!(error.to_string().contains("必须提供有限 region"));
}

#[test]
fn empty_strict_void_world_uses_one_block_origin_region() {
    let directory = TestDirectory::new();
    write_world_settings(directory.path(), true);
    let mut resolver = TestResolver::default();
    let loaded = StructureLoader::load(directory.path(), BlockPos::ZERO, &mut resolver).unwrap();
    assert_eq!(loaded.region_min, BlockPos::ZERO);
    assert_eq!(loaded.region_max, BlockPos::ZERO);
    assert_eq!(loaded.world.non_air_blocks(), 0);
}

fn block_chunk() -> Vec<u8> {
    let section = HashMap::from([
        ("Y".to_owned(), Value::Byte(0)),
        (
            "block_states".to_owned(),
            Value::Compound(HashMap::from([(
                "palette".to_owned(),
                Value::List(vec![block_state("minecraft:redstone_block")]),
            )])),
        ),
    ]);
    fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("sections".to_owned(), Value::List(vec![Value::Compound(section)])),
        (
            "block_entities".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                ("id".to_owned(), Value::String("minecraft:hopper".to_owned())),
                ("x".to_owned(), Value::Int(0)),
                ("y".to_owned(), Value::Int(0)),
                ("z".to_owned(), Value::Int(0)),
                ("TransferCooldown".to_owned(), Value::Int(4)),
            ]))]),
        ),
    ]))
    .unwrap()
}

fn entity_chunk() -> Vec<u8> {
    fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "Entities".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                ("id".to_owned(), Value::String("minecraft:item".to_owned())),
                (
                    "Pos".to_owned(),
                    Value::List(vec![
                        Value::Double(0.5),
                        Value::Double(1.0),
                        Value::Double(0.5),
                    ]),
                ),
            ]))]),
        ),
    ]))
    .unwrap()
}

fn write_world_settings(path: &Path, strict_void: bool) {
    let generator = if strict_void {
        HashMap::from([
            (
                "type".to_owned(),
                Value::String("minecraft:flat".to_owned()),
            ),
            (
                "settings".to_owned(),
                Value::Compound(HashMap::from([
                    (
                        "biome".to_owned(),
                        Value::String("minecraft:the_void".to_owned()),
                    ),
                    (
                        "layers".to_owned(),
                        Value::List(vec![Value::Compound(HashMap::from([
                            ("height".to_owned(), Value::Int(1)),
                            (
                                "block".to_owned(),
                                Value::String("minecraft:air".to_owned()),
                            ),
                        ]))]),
                    ),
                    (
                        "structure_overrides".to_owned(),
                        Value::List(Vec::new()),
                    ),
                    ("lakes".to_owned(), Value::Byte(0)),
                ])),
            ),
        ])
    } else {
        HashMap::from([(
            "type".to_owned(),
            Value::String("minecraft:noise".to_owned()),
        )])
    };
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "data".to_owned(),
            Value::Compound(HashMap::from([(
                "dimensions".to_owned(),
                Value::Compound(HashMap::from([(
                    "minecraft:overworld".to_owned(),
                    Value::Compound(HashMap::from([(
                        "generator".to_owned(),
                        Value::Compound(generator),
                    )])),
                )])),
            )])),
        ),
    ]);
    let data = fastnbt::to_bytes(&root).unwrap();
    let target = path.join("data/world_gen_settings.dat");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&data).unwrap();
    std::fs::write(target, encoder.finish().unwrap()).unwrap();
}

fn write_region(path: &Path, index: usize, chunk: Vec<u8>) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut bytes = vec![0u8; 8192 + 4096];
    let base = index * 4;
    bytes[base..base + 4].copy_from_slice(&[0, 0, 2, 1]);
    let length = (chunk.len() + 1) as u32;
    bytes[8192..8196].copy_from_slice(&length.to_be_bytes());
    bytes[8196] = 3;
    bytes[8197..8197 + chunk.len()].copy_from_slice(&chunk);
    std::fs::write(path, bytes).unwrap();
}

fn block_state(name: &str) -> Value {
    Value::Compound(HashMap::from([(
        "Name".to_owned(),
        Value::String(name.to_owned()),
    )]))
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-world-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
