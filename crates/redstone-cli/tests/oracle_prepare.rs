use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::{ByteArray, LongArray, Value};
use redstone_io::{Scenario, StructureFormat, StructureLoader};

#[test]
fn convert_scenario_converts_all_supported_structure_formats_to_vanilla_nbt() {
    let directory = TestDirectory::new();
    let litematic = directory.path().join("source.litematic");
    let schematic = directory.path().join("first.schem");
    let vanilla = directory.path().join("second.nbt");
    let scenario = directory.path().join("scenario.toml");
    let prepared = directory.path().join("prepared/scenario.toml");
    fs::write(&litematic, litematic_structure()).unwrap();
    fs::write(&schematic, sponge_structure()).unwrap();
    fs::write(&vanilla, vanilla_structure()).unwrap();
    fs::write(&scenario, scenario_text()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("convert")
        .arg(&scenario)
        .arg(&prepared)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let prepared = Scenario::load(&prepared).unwrap();
    let paths = std::iter::once(&prepared.source.path)
        .chain(prepared.source.pastes.iter().map(|paste| &paste.path))
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 3);
    for path in paths {
        assert!(path.is_file(), "转换结构不存在: {}", path.display());
        assert_eq!(
            StructureLoader::detect(path).unwrap(),
            StructureFormat::VanillaStructure
        );
        let root = fs::read(path).unwrap();
        let root = fastnbt::from_bytes::<HashMap<String, Value>>(&root).unwrap();
        assert_eq!(root.get("DataVersion"), Some(&Value::Int(4790)));
        assert!(matches!(root.get("palette"), Some(Value::List(values)) if !values.is_empty()));
        assert!(matches!(root.get("blocks"), Some(Value::List(values)) if !values.is_empty()));
    }
}

fn litematic_structure() -> Vec<u8> {
    let region = HashMap::from([
        ("Position".to_owned(), xyz(0, 0, 0)),
        ("Size".to_owned(), xyz(1, 1, 1)),
        (
            "BlockStatePalette".to_owned(),
            Value::List(vec![block_state("minecraft:redstone_block")]),
        ),
        (
            "BlockStates".to_owned(),
            Value::LongArray(LongArray::new(vec![0])),
        ),
        ("TileEntities".to_owned(), Value::List(Vec::new())),
        ("Entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&HashMap::from([
        ("Version".to_owned(), Value::Int(7)),
        ("MinecraftDataVersion".to_owned(), Value::Int(4790)),
        (
            "Regions".to_owned(),
            Value::Compound(HashMap::from([(
                "main".to_owned(),
                Value::Compound(region),
            )])),
        ),
    ]))
    .unwrap()
}

fn sponge_structure() -> Vec<u8> {
    fastnbt::to_bytes(&HashMap::from([
        ("Version".to_owned(), Value::Int(2)),
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("Width".to_owned(), Value::Short(1)),
        ("Height".to_owned(), Value::Short(1)),
        ("Length".to_owned(), Value::Short(1)),
        ("PaletteMax".to_owned(), Value::Int(1)),
        (
            "Palette".to_owned(),
            Value::Compound(HashMap::from([(
                "minecraft:redstone_block".to_owned(),
                Value::Int(0),
            )])),
        ),
        (
            "BlockData".to_owned(),
            Value::ByteArray(ByteArray::new(vec![0])),
        ),
    ]))
    .unwrap()
}

fn vanilla_structure() -> Vec<u8> {
    fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![block_state("minecraft:redstone_block")]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                (
                    "pos".to_owned(),
                    Value::List(vec![Value::Int(0), Value::Int(0), Value::Int(0)]),
                ),
                ("state".to_owned(), Value::Int(0)),
            ]))]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]))
    .unwrap()
}

fn block_state(name: &str) -> Value {
    Value::Compound(HashMap::from([(
        "Name".to_owned(),
        Value::String(name.to_owned()),
    )]))
}

fn xyz(x: i32, y: i32, z: i32) -> Value {
    Value::Compound(HashMap::from([
        ("x".to_owned(), Value::Int(x)),
        ("y".to_owned(), Value::Int(y)),
        ("z".to_owned(), Value::Int(z)),
    ]))
}

fn scenario_text() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
max_ticks = 1
strict = true

[source]
path = "source.litematic"
initialization = "raw"

[[source.pastes]]
path = "first.schem"
origin = { x = 2, y = 0, z = 0 }
ignore_air = true

[[source.pastes]]
path = "second.nbt"
tick = 1
origin = { x = 4, y = 0, z = 0 }
"#
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-convert-scenario-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
