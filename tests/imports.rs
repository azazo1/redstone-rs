use std::collections::BTreeMap;
use std::io::Write;

use fastnbt::{to_bytes, ByteArray, LongArray};
use flate2::{write::{GzEncoder, ZlibEncoder}, Compression};
use redstone_rs::{core::BlockKind, io::StructureInput, Position};
use serde::Serialize;

#[derive(Serialize)]
struct SpongeFixture {
    #[serde(rename = "Width")]
    width: i16,
    #[serde(rename = "Height")]
    height: i16,
    #[serde(rename = "Length")]
    length: i16,
    #[serde(rename = "Palette")]
    palette: BTreeMap<String, i32>,
    #[serde(rename = "BlockData")]
    block_data: ByteArray,
}

#[derive(Serialize)]
struct LitematicFixture {
    #[serde(rename = "Regions")]
    regions: BTreeMap<String, RegionFixture>,
}

#[derive(Serialize)]
struct RegionFixture {
    #[serde(rename = "Position")]
    position: Vec3Fixture,
    #[serde(rename = "Size")]
    size: Vec3Fixture,
    #[serde(rename = "BlockStatePalette")]
    palette: Vec<PaletteFixture>,
    #[serde(rename = "BlockStates")]
    block_states: LongArray,
}

#[derive(Serialize)]
struct Vec3Fixture {
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Serialize)]
struct PaletteFixture {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties")]
    properties: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct ChunkFixture {
    #[serde(rename = "xPos")]
    x: i32,
    #[serde(rename = "zPos")]
    z: i32,
    sections: Vec<ChunkSectionFixture>,
}

#[derive(Serialize)]
struct ChunkSectionFixture {
    #[serde(rename = "Y")]
    y: i8,
    block_states: ChunkBlockStatesFixture,
}

#[derive(Serialize)]
struct ChunkBlockStatesFixture {
    palette: Vec<PaletteFixture>,
    data: LongArray,
}

#[test]
fn imports_sponge_schematic_varint_block_data() {
    let mut palette = BTreeMap::new();
    palette.insert("minecraft:air".to_owned(), 0);
    palette.insert("minecraft:redstone_wire[power=7]".to_owned(), 1);
    let bytes = to_bytes(&SpongeFixture {
        width: 2,
        height: 1,
        length: 1,
        palette,
        block_data: ByteArray::new(vec![0, 1]),
    })
    .expect("fixture should serialize");

    let structure = StructureInput::from_nbt(&bytes).expect("sponge structure should decode");

    assert_eq!(structure.blocks.len(), 1);
    assert_eq!(structure.blocks[0].position, Position::new(1, 0, 0));
    assert_eq!(structure.blocks[0].state.kind, BlockKind::RedstoneWire);
    assert_eq!(structure.blocks[0].state.power(), 7);
}

#[test]
fn imports_gzip_litematic_with_negative_region_size() {
    let mut lever_properties = BTreeMap::new();
    lever_properties.insert("facing".to_owned(), "east".to_owned());
    lever_properties.insert("powered".to_owned(), "true".to_owned());
    let fixture = LitematicFixture {
        regions: BTreeMap::from([(
            "main".to_owned(),
            RegionFixture {
                position: Vec3Fixture { x: 10, y: 20, z: 30 },
                size: Vec3Fixture { x: -2, y: 1, z: 1 },
                palette: vec![
                    PaletteFixture {
                        name: "minecraft:air".to_owned(),
                        properties: BTreeMap::new(),
                    },
                    PaletteFixture {
                        name: "minecraft:lever".to_owned(),
                        properties: lever_properties,
                    },
                ],
                block_states: LongArray::new(vec![4]),
            },
        )]),
    };
    let raw = to_bytes(&fixture).expect("fixture should serialize");
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).expect("fixture should compress");
    let bytes = encoder.finish().expect("fixture should finish");

    let structure = StructureInput::from_nbt(&bytes).expect("litematic should decode");

    assert_eq!(structure.blocks.len(), 1);
    assert_eq!(structure.blocks[0].position, Position::new(9, 20, 30));
    assert_eq!(structure.blocks[0].state.kind, BlockKind::Lever);
    assert!(structure.blocks[0].state.powered());
}

#[tokio::test]
async fn imports_anvil_region_chunk_with_paletted_section() {
    let chunk = ChunkFixture {
        x: 2,
        z: -3,
        sections: vec![ChunkSectionFixture {
            y: 0,
            block_states: ChunkBlockStatesFixture {
                palette: vec![
                    PaletteFixture {
                        name: "minecraft:air".to_owned(),
                        properties: BTreeMap::new(),
                    },
                    PaletteFixture {
                        name: "minecraft:redstone_block".to_owned(),
                        properties: BTreeMap::new(),
                    },
                ],
                data: LongArray::new(vec![16].into_iter().chain(std::iter::repeat_n(0, 255)).collect()),
            },
        }],
    };
    let raw = to_bytes(&chunk).expect("chunk should serialize");
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(&raw).expect("chunk should compress");
    let payload = encoder.finish().expect("chunk should finish");
    assert!(payload.len() + 5 <= 4_096);

    let mut region = vec![0u8; 3 * 4_096];
    region[0..4].copy_from_slice(&[0, 0, 2, 1]);
    let length = (payload.len() + 1) as u32;
    region[8_192..8_196].copy_from_slice(&length.to_be_bytes());
    region[8_196] = 2;
    region[8_197..8_197 + payload.len()].copy_from_slice(&payload);
    let path = std::env::temp_dir().join(format!("redstone-rs-import-{}.mca", std::process::id()));
    std::fs::write(&path, region).expect("region fixture should write");

    let structure = StructureInput::from_path(&path).await.expect("anvil region should decode");
    std::fs::remove_file(&path).expect("region fixture should remove");

    assert_eq!(structure.blocks.len(), 1);
    assert_eq!(structure.blocks[0].position, Position::new(33, 0, -48));
    assert_eq!(structure.blocks[0].state.kind, BlockKind::RedstoneBlock);
}
