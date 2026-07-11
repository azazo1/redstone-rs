use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

fn main() {
    generate_item_stack_sizes();
    generate_block_traits();
}

fn generate_item_stack_sizes() {
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

fn generate_block_traits() {
    const VERSION: &str = "26.1.2";
    const DATA_VERSION: i64 = 4790;
    const DIRECTION_ORDER: [&str; 6] = ["west", "east", "down", "up", "north", "south"];

    let blocks_path = PathBuf::from("data/26.1.2/reports/blocks.json");
    let traits_path = PathBuf::from("data/26.1.2/reports/block-traits.json");
    println!("cargo:rerun-if-changed={}", blocks_path.display());
    println!("cargo:rerun-if-changed={}", traits_path.display());

    let blocks = read_json(&blocks_path, "official block report");
    let mut official_ids = Vec::new();
    for entry in blocks
        .as_object()
        .expect("official block report must be an object")
        .values()
    {
        for state in entry
            .get("states")
            .and_then(Value::as_array)
            .expect("official block entry must contain states")
        {
            let id = state
                .get("id")
                .and_then(Value::as_u64)
                .expect("official block state must contain an integer id")
                as usize;
            official_ids.push(id);
        }
    }
    official_ids.sort_unstable();
    for (expected, actual) in official_ids.iter().copied().enumerate() {
        assert_eq!(actual, expected, "official block state ids must be contiguous");
    }

    let report = read_json(&traits_path, "block traits report");
    assert_eq!(
        report.get("version").and_then(Value::as_str),
        Some(VERSION),
        "block traits report version must match"
    );
    assert_eq!(
        report.get("data_version").and_then(Value::as_i64),
        Some(DATA_VERSION),
        "block traits report data version must match"
    );
    let direction_order = report
        .get("direction_order")
        .and_then(Value::as_array)
        .expect("block traits report must contain direction_order")
        .iter()
        .map(|value| {
            value
                .as_str()
                .expect("block traits direction must be a string")
        })
        .collect::<Vec<_>>();
    assert_eq!(
        direction_order, DIRECTION_ORDER,
        "block traits direction order must match Rust"
    );

    let states = report
        .get("states")
        .and_then(Value::as_array)
        .expect("block traits report must contain states");
    assert_eq!(
        states.len(),
        official_ids.len(),
        "block traits report must cover every official state"
    );
    let mut packed = Vec::with_capacity(states.len() * 4);
    for (expected_id, state) in states.iter().enumerate() {
        let id = state
            .get("id")
            .and_then(Value::as_u64)
            .expect("block trait state must contain an integer id")
            as usize;
        assert_eq!(id, expected_id, "block trait state ids must be contiguous");
        assert_eq!(
            official_ids[expected_id], id,
            "block trait state id must exist in the official report"
        );
        packed.push(
            state
                .get("redstone_conductor")
                .and_then(Value::as_bool)
                .expect("block trait state must contain redstone_conductor")
                as u8,
        );
        for field in ["full_support", "center_support", "rigid_support"] {
            let mask = state
                .get(field)
                .and_then(Value::as_u64)
                .unwrap_or_else(|| panic!("block trait state must contain {field}"));
            assert!(mask <= 0b11_1111, "{field} must be a six-bit mask");
            packed.push(mask as u8);
        }
    }

    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"))
        .join("block_traits.bin");
    fs::write(output, packed).expect("packed block traits must be writable");
}

fn read_json(path: &Path, description: &str) -> Value {
    serde_json::from_str(
        &fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("{description} must be readable at {}: {error}", path.display())
        }),
    )
    .unwrap_or_else(|error| {
        panic!("{description} must be valid JSON at {}: {error}", path.display())
    })
}
