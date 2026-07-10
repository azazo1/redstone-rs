use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;

#[test]
#[ignore = "需要先执行 just oracle-build"]
fn real_java_oracle_matches_a_basic_action_scenario() {
    let directory = TestDirectory::new("oracle-basic");
    let structure_path = directory.path().join("machine.nbt");
    let scenario_path = directory.path().join("scenario.toml");
    fs::write(&structure_path, structure()).unwrap();
    fs::write(&scenario_path, scenario()).unwrap();

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

fn structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(1), Value::Int(1), Value::Int(1)]),
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
                    Value::List(vec![Value::Int(0), Value::Int(0), Value::Int(0)]),
                ),
                ("state".to_owned(), Value::Int(0)),
            ]))]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn scenario() -> &'static str {
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
