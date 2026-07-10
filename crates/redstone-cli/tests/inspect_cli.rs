use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;

#[test]
fn inspect_queries_block_types_coordinates_and_raw_components_as_json() {
    let directory = TestDirectory::new("inspect");
    let structure_path = directory.path().join("machine.nbt");
    fs::write(&structure_path, structure()).unwrap();

    let hopper_output = inspect(&structure_path, &["--type", "hopper", "--format", "json"]);
    assert_success(&hopper_output);
    let hopper_report = serde_json::from_slice::<serde_json::Value>(&hopper_output.stdout).unwrap();
    let hopper = &hopper_report["blocks"][0];
    assert_eq!(hopper_report["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(
        hopper["position"],
        serde_json::json!({ "x": 0, "y": 0, "z": 0 })
    );
    assert_eq!(hopper["state"]["name"], "minecraft:hopper");
    assert_eq!(hopper["state"]["properties"]["facing"], "down");
    assert_eq!(hopper["block_entity"]["id"], "minecraft:hopper");
    assert_eq!(
        hopper["block_entity"]["nbt"]["components"]["minecraft:custom_name"],
        "input"
    );
    assert_eq!(
        hopper["block_entity"]["nbt"]["Items"][0]["components"]["minecraft:custom_data"]["channel"],
        7
    );

    let stone_output = inspect(&structure_path, &["--block", "1,0,0", "--json"]);
    assert_success(&stone_output);
    let stone_report = serde_json::from_slice::<serde_json::Value>(&stone_output.stdout).unwrap();
    assert_eq!(stone_report["blocks"].as_array().unwrap().len(), 1);
    assert_eq!(
        stone_report["blocks"][0]["state"]["name"],
        "minecraft:stone"
    );
    assert!(stone_report["blocks"][0].get("block_entity").is_none());
}

#[test]
fn inspect_queries_real_litematic_block_entities_by_type() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let structure_path = workspace.join("assets/schematics/one-chooser.litematic");

    let output = inspect(&structure_path, &["--type", "dropper", "--json"]);
    assert_success(&output);
    let report = serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap();
    let blocks = report["blocks"].as_array().unwrap();
    assert_eq!(report["format"], "litematic");
    assert_eq!(blocks.len(), 16);
    assert!(blocks.iter().all(|block| {
        block["state"]["name"] == "minecraft:dropper"
            && block["block_entity"]["nbt"]["components"].is_object()
            && block["block_entity"]["nbt"]["Items"].is_array()
    }));
}

fn inspect(path: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("inspect")
        .arg(path)
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(2), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![
                block_state(
                    "minecraft:hopper",
                    &[("enabled", "true"), ("facing", "down")],
                ),
                block_state("minecraft:stone", &[]),
            ]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![
                structure_block(0, 0, Some(hopper_nbt())),
                structure_block(1, 1, None),
            ]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn block_state(name: &str, properties: &[(&str, &str)]) -> Value {
    let mut state = HashMap::from([("Name".to_owned(), Value::String(name.to_owned()))]);
    if !properties.is_empty() {
        state.insert(
            "Properties".to_owned(),
            Value::Compound(
                properties
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
                    .collect(),
            ),
        );
    }
    Value::Compound(state)
}

fn structure_block(x: i32, state: i32, nbt: Option<HashMap<String, Value>>) -> Value {
    let mut block = HashMap::from([
        (
            "pos".to_owned(),
            Value::List(vec![Value::Int(x), Value::Int(0), Value::Int(0)]),
        ),
        ("state".to_owned(), Value::Int(state)),
    ]);
    if let Some(nbt) = nbt {
        block.insert("nbt".to_owned(), Value::Compound(nbt));
    }
    Value::Compound(block)
}

fn hopper_nbt() -> HashMap<String, Value> {
    HashMap::from([
        (
            "id".to_owned(),
            Value::String("minecraft:hopper".to_owned()),
        ),
        (
            "components".to_owned(),
            Value::Compound(HashMap::from([(
                "minecraft:custom_name".to_owned(),
                Value::String("input".to_owned()),
            )])),
        ),
        (
            "Items".to_owned(),
            Value::List(vec![Value::Compound(HashMap::from([
                ("Slot".to_owned(), Value::Byte(0)),
                (
                    "id".to_owned(),
                    Value::String("minecraft:redstone".to_owned()),
                ),
                ("count".to_owned(), Value::Int(1)),
                (
                    "components".to_owned(),
                    Value::Compound(HashMap::from([(
                        "minecraft:custom_data".to_owned(),
                        Value::Compound(HashMap::from([("channel".to_owned(), Value::Int(7))])),
                    )])),
                ),
            ]))]),
        ),
    ])
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
