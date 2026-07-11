use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::{ByteArray, IntArray, Value};

#[test]
fn source_paste_before_simulation_skips_air_and_updates_the_pasted_selection() {
    run_paste_case("source-paste-setup", None);
}

#[test]
fn source_paste_at_tick_skips_air_and_updates_the_pasted_selection() {
    run_paste_case("source-paste-tick", Some(1));
}

fn run_paste_case(name: &str, tick: Option<u64>) {
    let directory = TestDirectory::new(name);
    let main_path = directory.path().join("computer.nbt");
    let rom_path = directory.path().join("rom.schem");
    let scenario_path = directory.path().join("scenario.toml");
    let trace_path = directory.path().join("trace.jsonl");
    fs::write(&main_path, main_structure()).unwrap();
    fs::write(&rom_path, rom_schematic()).unwrap();
    fs::write(&scenario_path, scenario(tick)).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario_path)
        .arg("--trace")
        .arg(&trace_path)
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("expectations: passed"));
    let trace = fs::read_to_string(trace_path).unwrap();
    assert!(trace.lines().any(|line| {
        let event = serde_json::from_str::<serde_json::Value>(line).unwrap();
        event["tick"] == tick.unwrap_or(0)
            && event["event"] == "block_changed"
            && event["cause"] == "structure_paste"
            && event["pos"] == serde_json::json!({ "x": 0, "y": 0, "z": 0 })
    }));
}

fn main_structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(2), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![block_state(
                "minecraft:redstone_lamp",
                &[("lit", "false")],
            )]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                (
                    "pos".to_owned(),
                    Value::List(vec![Value::Int(1), Value::Int(0), Value::Int(0)]),
                ),
                ("state".to_owned(), Value::Int(0)),
            ]))]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn rom_schematic() -> Vec<u8> {
    let root = HashMap::from([
        ("Version".to_owned(), Value::Int(2)),
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("Width".to_owned(), Value::Short(2)),
        ("Height".to_owned(), Value::Short(1)),
        ("Length".to_owned(), Value::Short(1)),
        (
            "Offset".to_owned(),
            Value::IntArray(IntArray::new(vec![0, 0, 0])),
        ),
        ("PaletteMax".to_owned(), Value::Int(2)),
        (
            "Palette".to_owned(),
            Value::Compound(HashMap::from([
                ("minecraft:redstone_block".to_owned(), Value::Int(0)),
                ("minecraft:air".to_owned(), Value::Int(1)),
            ])),
        ),
        (
            "BlockData".to_owned(),
            Value::ByteArray(ByteArray::new(vec![0, 1])),
        ),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn block_state(name: &str, properties: &[(&str, &str)]) -> Value {
    Value::Compound(HashMap::from([
        ("Name".to_owned(), Value::String(name.to_owned())),
        (
            "Properties".to_owned(),
            Value::Compound(
                properties
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
                    .collect(),
            ),
        ),
    ]))
}

fn scenario(tick: Option<u64>) -> String {
    let tick = tick.map_or_else(String::new, |tick| format!("tick = {tick}\n"));
    format!(
        r#"version = "26.1.2"
mode = "default"
max_ticks = 1
strict = true

[source]
path = "computer.nbt"
initialization = "raw"

[[source.pastes]]
path = "rom.schem"
{tick}origin = {{ x = 0, y = 0, z = 0 }}
ignore_air = true
paste_entities = false
update = true

[[probes]]
name = "rom_lamp"
type = "property"
pos = {{ x = 1, y = 0, z = 0 }}
property = "lit"

[[expectations]]
tick = 1
probe = "rom_lamp"
equals = "true"
"#
    )
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-cli-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
