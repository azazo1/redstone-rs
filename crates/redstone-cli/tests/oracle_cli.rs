use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_a_basic_action_scenario() {
    assert_oracle_matches("oracle-basic", structure(1, 0), basic_scenario());
}

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_transformed_coordinates() {
    assert_oracle_matches("oracle-transform", structure(2, 1), transformed_scenario());
}

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_notify_initialization() {
    assert_oracle_matches("oracle-notify", powered_wire_structure(), notify_scenario());
}

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_experimental_mode_and_seed() {
    assert_oracle_matches(
        "oracle-experimental",
        powered_wire_structure(),
        experimental_scenario(),
    );
}

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_target_scheduled_release() {
    assert_oracle_matches("oracle-target", target_structure(), target_scenario());
}

fn assert_oracle_matches(name: &str, structure: Vec<u8>, scenario: &str) {
    let directory = TestDirectory::new(name);
    let structure_path = directory.path().join("machine.nbt");
    let scenario_path = directory.path().join("scenario.toml");
    fs::write(&structure_path, structure).unwrap();
    fs::write(&scenario_path, scenario).unwrap();

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let oracle = workspace.join("tools/vanilla-oracle/run.sh");
    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .current_dir(&workspace)
        .env("REDSTONE_ORACLE", &oracle)
        .arg("test")
        .arg(&scenario_path)
        .arg("--oracle")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8(output.stdout).unwrap().contains("PASS"));
}

fn structure(size_x: i32, block_x: i32) -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(size_x), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([(
                "Name".to_owned(),
                Value::String("minecraft:redstone_block".to_owned()),
            )]))]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                (
                    "pos".to_owned(),
                    Value::List(vec![Value::Int(block_x), Value::Int(0), Value::Int(0)]),
                ),
                ("state".to_owned(), Value::Int(0)),
            ]))]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn powered_wire_structure() -> Vec<u8> {
    let properties = HashMap::from([
        ("east".to_owned(), Value::String("none".to_owned())),
        ("north".to_owned(), Value::String("none".to_owned())),
        ("power".to_owned(), Value::String("0".to_owned())),
        ("south".to_owned(), Value::String("none".to_owned())),
        ("west".to_owned(), Value::String("none".to_owned())),
    ]);
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(2), Value::Int(2), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![
                Value::Compound(HashMap::from([(
                    "Name".to_owned(),
                    Value::String("minecraft:stone".to_owned()),
                )])),
                Value::Compound(HashMap::from([(
                    "Name".to_owned(),
                    Value::String("minecraft:redstone_block".to_owned()),
                )])),
                Value::Compound(HashMap::from([
                    (
                        "Name".to_owned(),
                        Value::String("minecraft:redstone_wire".to_owned()),
                    ),
                    ("Properties".to_owned(), Value::Compound(properties)),
                ])),
            ]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![
                structure_block(0, 0, 0, 0),
                structure_block(1, 0, 0, 0),
                structure_block(0, 1, 0, 1),
                structure_block(1, 1, 0, 2),
            ]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn target_structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                (
                    "Name".to_owned(),
                    Value::String("minecraft:target".to_owned()),
                ),
                (
                    "Properties".to_owned(),
                    Value::Compound(HashMap::from([(
                        "power".to_owned(),
                        Value::String("0".to_owned()),
                    )])),
                ),
            ]))]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![structure_block(0, 0, 0, 0)]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn structure_block(x: i32, y: i32, z: i32, state: i32) -> Value {
    Value::Compound(HashMap::from([
        (
            "pos".to_owned(),
            Value::List(vec![Value::Int(x), Value::Int(y), Value::Int(z)]),
        ),
        ("state".to_owned(), Value::Int(state)),
    ]))
}

fn basic_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 2
strict = true

[source]
path = "machine.nbt"
initialization = "raw"

[[actions]]
tick = 2
type = "break_block"
pos = { x = 0, y = 0, z = 0 }

[[probes]]
name = "state"
type = "block_state"
pos = { x = 0, y = 0, z = 0 }
"#
}

fn transformed_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 2
strict = true

[source]
path = "machine.nbt"
origin = { x = 10, y = 3, z = 20 }
initialization = "raw"
rotation = "clockwise90"
mirror = "front_back"

[[actions]]
tick = 2
type = "break_block"
pos = { x = 10, y = 3, z = 19 }

[[probes]]
name = "state"
type = "block_state"
pos = { x = 10, y = 3, z = 19 }
"#
}

fn notify_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 2
strict = true

[source]
path = "machine.nbt"
initialization = "notify"

[[probes]]
name = "wire_power"
type = "property"
pos = { x = 1, y = 1, z = 0 }
property = "power"
"#
}

fn experimental_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "experimental"
seed = 42
max_ticks = 2
strict = true

[source]
path = "machine.nbt"
initialization = "notify"

[[probes]]
name = "wire_power"
type = "property"
pos = { x = 1, y = 1, z = 0 }
property = "power"
"#
}

fn target_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 9
strict = true

[source]
path = "machine.nbt"
initialization = "raw"

[[actions]]
tick = 1
type = "hit_target"
pos = { x = 0, y = 0, z = 0 }
face = "north"
location = [0.9, 0.5, 0.0]
arrow = false

[[probes]]
name = "power"
type = "property"
pos = { x = 0, y = 0, z = 0 }
property = "power"
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
