use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use fastnbt::Value;
use flate2::read::GzDecoder;

use super::super::value_i32;

pub(super) const DATA_VERSION: i32 = 4790;

pub(super) fn read(path: &Path) -> Result<HashMap<String, Value>, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))?;
    let mut decoded = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut decoded)
            .map_err(|error| error.to_string())?;
    } else {
        decoded = bytes;
    }
    fastnbt::from_bytes(&decoded).map_err(|error| error.to_string())
}

pub(super) fn require_data_version(
    root: &HashMap<String, Value>,
    path: &Path,
) -> Result<(), String> {
    let version = root.get("DataVersion").and_then(value_i32);
    if version != Some(DATA_VERSION) {
        return Err(format!(
            "{} 的 DataVersion 必须为 {DATA_VERSION}, 收到 {version:?}",
            path.display()
        ));
    }
    Ok(())
}

pub(super) fn is_strict_void(root: &HashMap<String, Value>) -> bool {
    let Some(Value::Compound(data)) = root.get("data") else {
        return false;
    };
    let Some(Value::Compound(dimensions)) = data.get("dimensions") else {
        return false;
    };
    let Some(Value::Compound(overworld)) = dimensions.get("minecraft:overworld") else {
        return false;
    };
    let Some(Value::Compound(generator)) = overworld.get("generator") else {
        return false;
    };
    if !matches!(generator.get("type"), Some(Value::String(value)) if value == "minecraft:flat") {
        return false;
    }
    let Some(Value::Compound(settings)) = generator.get("settings") else {
        return false;
    };
    if !matches!(settings.get("biome"), Some(Value::String(value)) if value == "minecraft:the_void") {
        return false;
    }
    if settings
        .get("lakes")
        .is_some_and(|value| !matches!(value, Value::Byte(0)))
    {
        return false;
    }
    if !matches!(settings.get("structure_overrides"), Some(Value::List(values)) if values.is_empty()) {
        return false;
    }
    matches!(settings.get("layers"), Some(Value::List(layers)) if !layers.is_empty() && layers.iter().all(|layer| {
        matches!(layer, Value::Compound(layer) if matches!(layer.get("block"), Some(Value::String(block)) if matches!(block.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_void_requires_the_void_air_layers_and_empty_structures() {
        let root = HashMap::from([
            ("DataVersion".to_owned(), Value::Int(DATA_VERSION)),
            (
                "data".to_owned(),
                Value::Compound(HashMap::from([(
                    "dimensions".to_owned(),
                    Value::Compound(HashMap::from([(
                        "minecraft:overworld".to_owned(),
                        Value::Compound(HashMap::from([(
                            "generator".to_owned(),
                            Value::Compound(HashMap::from([
                                (
                                    "type".to_owned(),
                                    Value::String("minecraft:flat".to_owned()),
                                ),
                                (
                                    "settings".to_owned(),
                                    Value::Compound(HashMap::from([
                                        (
                                            "biome".to_owned(),
                                            Value::String("minecraft:the_void".to_owned()),
                                        ),
                                        (
                                            "layers".to_owned(),
                                            Value::List(vec![Value::Compound(HashMap::from([(
                                                "block".to_owned(),
                                                Value::String("minecraft:air".to_owned()),
                                            )]))]),
                                        ),
                                        (
                                            "structure_overrides".to_owned(),
                                            Value::List(Vec::new()),
                                        ),
                                        ("lakes".to_owned(), Value::Byte(0)),
                                    ])),
                                ),
                            ])),
                        )])),
                    )])),
                )])),
            ),
        ]);
        assert!(is_strict_void(&root));
    }
}
