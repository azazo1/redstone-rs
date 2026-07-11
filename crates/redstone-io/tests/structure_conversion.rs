mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use common::TestResolver;
use redstone_core::{BlockEntityData, BlockPos, BlockStateId, EntityData, SparseWorld};
use redstone_io::{
    LoadedStructure, StructureLoader, StructureState, StructureWriteOptions, StructureWriter,
};

#[test]
fn all_structure_writers_round_trip_content() {
    let directory = TestDirectory::new();
    let structure = fixture();
    for extension in ["litematic", "schem", "nbt"] {
        let path = directory.path().join(format!("converted.{extension}"));
        StructureWriter::write(
            &path,
            &structure,
            StructureWriteOptions::new(4790),
            describe_state,
        )
        .unwrap();
        let mut resolver = TestResolver::default();
        let loaded = StructureLoader::load(&path, BlockPos::ZERO, &mut resolver).unwrap();

        assert_eq!(loaded.world.non_air_blocks(), 2, "{extension}");
        assert_eq!(loaded.region_max.x - loaded.region_min.x, 2, "{extension}");
        assert_eq!(loaded.world.block_entities().count(), 1, "{extension}");
        assert_eq!(loaded.world.entities().count(), 1, "{extension}");
        assert_eq!(
            loaded.world.entities().next().unwrap().1.kind,
            "minecraft:item_frame",
            "{extension}"
        );
    }
}

fn fixture() -> LoadedStructure {
    let mut world = SparseWorld::new(BlockStateId(0));
    world
        .set_block(BlockPos::new(5, 10, -2), BlockStateId(1))
        .unwrap();
    world
        .set_block(BlockPos::new(7, 10, -2), BlockStateId(2))
        .unwrap();
    world.set_block_entity(
        BlockPos::new(7, 10, -2),
        BlockEntityData {
            kind: "minecraft:hopper".to_owned(),
            fields: BTreeMap::from([("TransferCooldown".to_owned(), 7.into())]),
        },
    );
    world.spawn_entity(EntityData {
        kind: "minecraft:item_frame".to_owned(),
        position: [6.5, 10.0, -1.5],
        fields: BTreeMap::from([("Facing".to_owned(), 2.into())]),
    });
    LoadedStructure {
        world,
        min: BlockPos::new(5, 10, -2),
        max: BlockPos::new(7, 10, -2),
        region_min: BlockPos::new(5, 10, -2),
        region_max: BlockPos::new(7, 10, -2),
        format: "test".to_owned(),
        data_version: Some(4790),
        block_counts: BTreeMap::from([
            ("minecraft:stone".to_owned(), 1),
            ("minecraft:repeater".to_owned(), 1),
        ]),
        block_entity_nbt: BTreeMap::new(),
    }
}

fn describe_state(id: BlockStateId) -> Result<StructureState, String> {
    Ok(match id.0 {
        0 => StructureState {
            name: "minecraft:air".to_owned(),
            properties: BTreeMap::new(),
        },
        1 => StructureState {
            name: "minecraft:stone".to_owned(),
            properties: BTreeMap::new(),
        },
        2 => StructureState {
            name: "minecraft:repeater".to_owned(),
            properties: BTreeMap::from([
                ("delay".to_owned(), "1".to_owned()),
                ("facing".to_owned(), "north".to_owned()),
                ("locked".to_owned(), "false".to_owned()),
                ("powered".to_owned(), "false".to_owned()),
            ]),
        },
        value => return Err(format!("未知测试状态 {value}")),
    })
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-structure-conversion-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
