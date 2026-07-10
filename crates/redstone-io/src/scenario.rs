use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use redstone_core::{
    BlockEntityData, BlockPos, EntityData, EntityId, Expectation, GameTick, Probe, ProbeValue,
    RedstoneMode,
};
use serde::Deserialize;
use thiserror::Error;

use crate::{Mirror, Rotation, StructureTransform};

#[derive(Clone, Debug, Deserialize)]
pub struct Scenario {
    pub version: String,
    pub mode: RedstoneMode,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "default_max_ticks")]
    pub max_ticks: u64,
    #[serde(default = "default_strict")]
    pub strict: bool,
    pub source: ScenarioSource,
    #[serde(default)]
    pub actions: Vec<ScenarioAction>,
    #[serde(default)]
    pub probes: Vec<ScenarioProbe>,
    #[serde(default)]
    pub expectations: Vec<ScenarioExpectation>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioSource {
    pub path: PathBuf,
    #[serde(default)]
    pub origin: BlockPos,
    #[serde(default)]
    pub initialization: InitializationMode,
    #[serde(default)]
    pub rotation: Rotation,
    #[serde(default)]
    pub mirror: Mirror,
}

impl ScenarioSource {
    pub fn transform(&self) -> StructureTransform {
        StructureTransform {
            origin: self.origin,
            rotation: self.rotation,
            mirror: self.mirror,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InitializationMode {
    Raw,
    #[default]
    Notify,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioAction {
    pub tick: GameTick,
    #[serde(flatten)]
    pub action: ScenarioActionKind,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScenarioActionKind {
    SetBlock {
        pos: BlockPos,
        name: String,
        #[serde(default)]
        properties: BTreeMap<String, String>,
    },
    BreakBlock {
        pos: BlockPos,
    },
    UseBlock {
        pos: BlockPos,
    },
    PressButton {
        pos: BlockPos,
    },
    PullLever {
        pos: BlockPos,
    },
    SetBlockEntity {
        pos: BlockPos,
        data: BlockEntityData,
    },
    SpawnEntity {
        #[serde(default)]
        id: Option<EntityId>,
        kind: String,
        position: [f64; 3],
        #[serde(default)]
        fields: BTreeMap<String, serde_json::Value>,
    },
    MoveEntity {
        id: EntityId,
        position: [f64; 3],
    },
    RemoveEntity {
        id: EntityId,
    },
    SetEntityField {
        id: EntityId,
        field: String,
        value: serde_json::Value,
    },
    HitTarget {
        pos: BlockPos,
        face: redstone_core::Direction,
        location: [f64; 3],
        #[serde(default)]
        arrow: bool,
    },
}

impl ScenarioActionKind {
    pub fn entity_data(kind: String, position: [f64; 3], fields: BTreeMap<String, serde_json::Value>) -> EntityData {
        EntityData {
            kind,
            position,
            fields,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioProbe {
    pub name: String,
    #[serde(flatten)]
    pub probe: Probe,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenarioExpectation {
    pub tick: GameTick,
    pub probe: String,
    pub equals: ProbeValue,
}

impl Scenario {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ScenarioError> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)?;
        let mut scenario: Self = toml::from_str(&text)?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        if scenario.source.path.is_relative() {
            scenario.source.path = parent.join(&scenario.source.path);
        }
        Ok(scenario)
    }

    pub fn expectations(&self) -> Vec<Expectation> {
        self.expectations
            .iter()
            .map(|expectation| Expectation {
                tick: expectation.tick,
                probe: expectation.probe.clone(),
                equals: expectation.equals.clone(),
            })
            .collect()
    }
}

fn default_max_ticks() -> u64 {
    100
}

fn default_strict() -> bool {
    true
}

#[derive(Debug, Error)]
pub enum ScenarioError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Toml(#[from] toml::de::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_probe_uses_a_distinct_property_key() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[source]
path = "machine.nbt"

[[probes]]
name = "output_powered"
type = "property"
pos = { x = 1, y = 2, z = 3 }
property = "powered"
"#,
        )
        .unwrap();

        assert!(matches!(
            &scenario.probes[0].probe,
            Probe::Property { property, .. } if property == "powered"
        ));
    }

    #[test]
    fn entity_actions_and_probes_parse_from_toml() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[source]
path = "machine.nbt"

[[actions]]
tick = 1
type = "spawn_entity"
id = 9
kind = "minecraft:hopper_minecart"
position = [0.5, 0.0, 0.5]
fields = { enabled = true, capacity = 320 }

[[actions]]
tick = 2
type = "move_entity"
id = 9
position = [1.5, 0.0, 0.5]

[[probes]]
name = "inventory"
type = "entity_container_count"
id = 9
"#,
        )
        .unwrap();

        assert!(matches!(
            &scenario.actions[0].action,
            ScenarioActionKind::SpawnEntity {
                id: Some(EntityId(9)),
                fields,
                ..
            } if fields["enabled"] == serde_json::Value::Bool(true)
        ));
        assert!(matches!(
            scenario.probes[0].probe,
            Probe::EntityContainerCount { id: EntityId(9) }
        ));
    }
}
