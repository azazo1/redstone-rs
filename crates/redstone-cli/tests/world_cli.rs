use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

#[test]
fn world_directory_supports_inspect_convert_and_scenario_loading() {
    let directory = TestDirectory::new();
    let world = directory.path().join("world");
    write_world(&world);

    let inspect = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("inspect")
        .arg(&world)
        .arg("--region")
        .arg("0..=15,0..=15,0..=15")
        .arg("--json")
        .output()
        .unwrap();
    assert_success(&inspect);
    let report: serde_json::Value = serde_json::from_slice(&inspect.stdout).unwrap();
    assert_eq!(report["format"], "minecraft_world");
    assert_eq!(report["region_bounds"]["max"]["x"], 15);

    let converted = directory.path().join("world.schem");
    let convert = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("convert")
        .arg(&world)
        .arg(&converted)
        .arg("--region")
        .arg("0..=15,0..=15,0..=15")
        .output()
        .unwrap();
    assert_success(&convert);
    assert!(converted.is_file());

    let archive = directory.path().join("world.zip");
    write_world_zip(&world, &archive, "saved-world");
    let inspect_zip = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("inspect")
        .arg(&archive)
        .arg("--region")
        .arg("0..=15,0..=15,0..=15")
        .arg("--json")
        .output()
        .unwrap();
    assert_success(&inspect_zip);
    let converted_zip = directory.path().join("world-from-zip.litematic");
    let convert_zip = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("convert")
        .arg(&archive)
        .arg(&converted_zip)
        .arg("--region")
        .arg("0..=15,0..=15,0..=15")
        .output()
        .unwrap();
    assert_success(&convert_zip);
    assert!(converted_zip.is_file());

    let scenario = directory.path().join("world.toml");
    std::fs::write(
        &scenario,
        format!(
            r#"version = "26.1.2"
mode = "default"
max_ticks = 1
strict = false

[source]
path = "{}"
initialization = "raw"
region = {{ min = {{ x = 0, y = 0, z = 0 }}, max = {{ x = 15, y = 15, z = 15 }} }}
"#,
            archive.display()
        ),
    )
    .unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario)
        .output()
        .unwrap();
    assert_success(&run);
}

fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_world(path: &Path) {
    let settings = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "data".to_owned(),
            Value::Compound(HashMap::from([(
                "dimensions".to_owned(),
                Value::Compound(HashMap::from([(
                    "minecraft:overworld".to_owned(),
                    Value::Compound(HashMap::from([(
                        "generator".to_owned(),
                        Value::Compound(HashMap::from([(
                            "type".to_owned(),
                            Value::String("minecraft:noise".to_owned()),
                        )])),
                    )])),
                )])),
            )])),
        ),
    ]);
    let settings_path = path.join("data/world_gen_settings.dat");
    std::fs::create_dir_all(settings_path.parent().unwrap()).unwrap();
    std::fs::write(settings_path, fastnbt::to_bytes(&settings).unwrap()).unwrap();

    let section = HashMap::from([
        ("Y".to_owned(), Value::Byte(0)),
        (
            "block_states".to_owned(),
            Value::Compound(HashMap::from([(
                "palette".to_owned(),
                Value::List(vec![Value::Compound(HashMap::from([(
                    "Name".to_owned(),
                    Value::String("minecraft:redstone_block".to_owned()),
                )]))]),
            )])),
        ),
    ]);
    let chunk = fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        ("sections".to_owned(), Value::List(vec![Value::Compound(section)])),
        ("block_entities".to_owned(), Value::List(Vec::new())),
    ]))
    .unwrap();
    let region_path = path.join("dimensions/minecraft/overworld/region/r.0.0.mca");
    std::fs::create_dir_all(region_path.parent().unwrap()).unwrap();
    let mut region = vec![0u8; 8192 + 4096];
    region[..4].copy_from_slice(&[0, 0, 2, 1]);
    region[8192..8196].copy_from_slice(&((chunk.len() + 1) as u32).to_be_bytes());
    region[8196] = 3;
    region[8197..8197 + chunk.len()].copy_from_slice(&chunk);
    std::fs::write(region_path, region).unwrap();
}

fn write_world_zip(world: &Path, output: &Path, prefix: &str) {
    let file = File::create(output).unwrap();
    let mut archive = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for relative in [
        "data/world_gen_settings.dat",
        "dimensions/minecraft/overworld/region/r.0.0.mca",
    ] {
        archive
            .start_file(format!("{prefix}/{relative}"), options)
            .unwrap();
        archive
            .write_all(&std::fs::read(world.join(relative)).unwrap())
            .unwrap();
    }
    archive.finish().unwrap();
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-world-cli-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}
