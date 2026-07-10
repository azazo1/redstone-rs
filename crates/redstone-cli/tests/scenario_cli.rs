use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;

#[test]
fn run_executes_entity_and_target_actions_through_the_cli() {
    let directory = TestDirectory::new("scenario-actions");
    let structure_path = directory.path().join("machine.nbt");
    let scenario_path = directory.path().join("scenario.toml");
    let trace_path = directory.path().join("trace.jsonl");
    let vcd_path = directory.path().join("signals.vcd");
    let repeated_trace_path = directory.path().join("trace-repeated.jsonl");
    let repeated_vcd_path = directory.path().join("signals-repeated.vcd");
    fs::write(&structure_path, structure()).unwrap();
    fs::write(&scenario_path, scenario()).unwrap();

    let output = run(&scenario_path, &trace_path, &vcd_path);

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ticks: 9"));
    assert!(stdout.contains("expectations: passed"));

    let action_types = fs::read_to_string(&trace_path)
        .unwrap()
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["tick"] == 1 && event["event"] == "action")
        .filter_map(|event| event["action"]["type"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    assert_eq!(
        action_types,
        ["spawn_entity", "set_entity_field", "hit_target"]
    );
    let vcd = fs::read_to_string(&vcd_path).unwrap();
    assert!(vcd.contains("$enddefinitions $end"));
    assert!(vcd.contains("target_power"));
    assert!(vcd.contains("plate_powered"));
    assert!(vcd.contains("entity_label"));

    let repeated = run(&scenario_path, &repeated_trace_path, &repeated_vcd_path);
    assert!(repeated.status.success());
    assert_eq!(fs::read(trace_path).unwrap(), fs::read(repeated_trace_path).unwrap());
    assert_eq!(fs::read(vcd_path).unwrap(), fs::read(repeated_vcd_path).unwrap());
}

fn run(scenario: &Path, trace: &Path, vcd: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(scenario)
        .arg("--trace")
        .arg(trace)
        .arg("--vcd")
        .arg(vcd)
        .output()
        .unwrap()
}

fn structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(3), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![
                block_state("minecraft:target", &[("power", "0")]),
                block_state("minecraft:oak_pressure_plate", &[("powered", "false")]),
            ]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![structure_block(0, 0), structure_block(2, 1)]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
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
                    .map(|(name, value)| {
                        ((*name).to_owned(), Value::String((*value).to_owned()))
                    })
                    .collect(),
            ),
        ),
    ]))
}

fn structure_block(x: i32, state: i32) -> Value {
    Value::Compound(HashMap::from([
        (
            "pos".to_owned(),
            Value::List(vec![Value::Int(x), Value::Int(0), Value::Int(0)]),
        ),
        ("state".to_owned(), Value::Int(state)),
    ]))
}

fn scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 7
max_ticks = 9
strict = true

[source]
path = "machine.nbt"
initialization = "raw"

[[actions]]
tick = 1
type = "spawn_entity"
id = 50
kind = "minecraft:generic_collision"
position = [4.5, 0.1, 0.5]
fields = { label = "initial" }

[[actions]]
tick = 1
type = "set_entity_field"
id = 50
field = "label"
value = "updated"

[[actions]]
tick = 1
type = "hit_target"
pos = { x = 0, y = 0, z = 0 }
face = "north"
location = [0.9, 0.5, 0.0]

[[actions]]
tick = 2
type = "move_entity"
id = 50
position = [2.5, 0.1, 0.5]

[[probes]]
name = "target_power"
type = "property"
pos = { x = 0, y = 0, z = 0 }
property = "power"

[[probes]]
name = "plate_powered"
type = "property"
pos = { x = 2, y = 0, z = 0 }
property = "powered"

[[probes]]
name = "entity_label"
type = "entity_field"
id = 50
field = "label"

[[expectations]]
tick = 1
probe = "target_power"
equals = "3"

[[expectations]]
tick = 1
probe = "entity_label"
equals = "updated"

[[expectations]]
tick = 2
probe = "plate_powered"
equals = "true"

[[expectations]]
tick = 9
probe = "target_power"
equals = "0"
"#
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

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
