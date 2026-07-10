use std::env;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn main() {
    let report_dir = PathBuf::from("data/26.1.2/reports/minecraft/components/item");
    let mut entries = fs::read_dir(&report_dir)
        .expect("official item component reports must exist")
        .map(|entry| {
            entry
                .expect("item component report entry must be readable")
                .path()
        })
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    entries.sort();
    println!("cargo:rerun-if-changed={}", report_dir.display());

    let mut generated = String::from(
        "pub(super) fn official_item_max_stack_size(item_id: &str) -> i64 {\n    match item_id.strip_prefix(\"minecraft:\").unwrap_or(item_id) {\n",
    );
    for path in entries {
        println!("cargo:rerun-if-changed={}", path.display());
        let report = serde_json::from_str::<Value>(
            &fs::read_to_string(&path).expect("item component report must be readable"),
        )
        .expect("item component report must be valid JSON");
        let max_stack_size = report
            .get("components")
            .and_then(|components| components.get("minecraft:max_stack_size"))
            .and_then(Value::as_i64)
            .expect("item component report must contain minecraft:max_stack_size");
        if max_stack_size == 64 {
            continue;
        }
        let item = path
            .file_stem()
            .and_then(|name| name.to_str())
            .expect("item component report filename must be UTF-8");
        generated.push_str(&format!("        \"{item}\" => {max_stack_size},\n"));
    }
    generated.push_str("        _ => 64,\n    }\n}\n");

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"))
        .join("item_stack_sizes.rs");
    fs::write(output, generated).expect("generated item stack sizes must be writable");
}
