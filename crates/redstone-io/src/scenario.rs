use std::path::{Path, PathBuf};

use std::collections::BTreeMap;

use redstone_core::{
    BlockEntityData, BlockPos, BlockStateId, EntityData, EntityId, Expectation, GameTick, Probe,
    ProbeValue, RedstoneMode,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{Mirror, Rotation, StructureTransform};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Scenario {
    pub version: String,
    pub mode: RedstoneMode,
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "default_max_ticks")]
    pub max_ticks: u64,
    #[serde(default = "default_strict")]
    pub strict: bool,
    #[serde(default)]
    pub oracle_micro_trace: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay: Option<ScenarioReplay>,
    pub source: ScenarioSource,
    #[serde(default)]
    pub actions: Vec<ScenarioAction>,
    #[serde(default)]
    pub probes: Vec<ScenarioProbe>,
    #[serde(default)]
    pub expectations: Vec<ScenarioExpectation>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ScenarioReplay {
    #[serde(default)]
    pub camera: ScenarioReplayCamera,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ScenarioReplayCamera {
    #[serde(default = "default_replay_view_distance")]
    pub view_distance: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yaw: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pitch: Option<f32>,
}

impl Default for ScenarioReplayCamera {
    fn default() -> Self {
        Self {
            view_distance: default_replay_view_distance(),
            position: None,
            yaw: None,
            pitch: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    #[serde(default)]
    pub pastes: Vec<ScenarioPaste>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScenarioPaste {
    pub path: PathBuf,
    #[serde(default)]
    pub tick: Option<GameTick>,
    #[serde(default)]
    pub origin: BlockPos,
    #[serde(default)]
    pub rotation: Rotation,
    #[serde(default)]
    pub mirror: Mirror,
    #[serde(default)]
    pub ignore_air: bool,
    #[serde(default)]
    pub paste_entities: bool,
    #[serde(default)]
    pub update: bool,
}

impl ScenarioPaste {
    pub fn transform(&self) -> StructureTransform {
        StructureTransform {
            origin: self.origin,
            rotation: self.rotation,
            mirror: self.mirror,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InitializationMode {
    Raw,
    #[default]
    Notify,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScenarioAction {
    pub tick: GameTick,
    #[serde(flatten)]
    pub action: ScenarioActionKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
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
    pub fn entity_data(
        kind: String,
        position: [f64; 3],
        fields: BTreeMap<String, serde_json::Value>,
    ) -> EntityData {
        EntityData {
            kind,
            position,
            fields,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScenarioProbe {
    pub name: String,
    #[serde(flatten)]
    pub probe: Probe,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScenarioExpectation {
    pub tick: GameTick,
    pub probe: String,
    pub equals: ScenarioExpectationValue,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ScenarioExpectationValue {
    State { state: BlockStateId },
    Value(ProbeValue),
}

impl ScenarioExpectationValue {
    fn probe_value(&self) -> ProbeValue {
        match self {
            Self::State { state } => ProbeValue::State(*state),
            Self::Value(value) => value.clone(),
        }
    }
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
        for paste in &mut scenario.source.pastes {
            if paste.path.is_relative() {
                paste.path = parent.join(&paste.path);
            }
        }
        Ok(scenario)
    }

    pub fn expectations(&self) -> Vec<Expectation> {
        self.expectations
            .iter()
            .map(|expectation| Expectation {
                tick: expectation.tick,
                probe: expectation.probe.clone(),
                equals: expectation.equals.probe_value(),
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

fn default_replay_view_distance() -> i32 {
    8
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
    fn state_expectation_uses_an_explicit_state_value() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[source]
path = "machine.nbt"

[[expectations]]
tick = 1
probe = "output"
equals = { state = 11323 }
"#,
        )
        .unwrap();

        assert_eq!(
            scenario.expectations()[0].equals,
            ProbeValue::State(BlockStateId(11323))
        );
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

    #[test]
    fn source_pastes_parse_worldedit_style_options() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[source]
path = "computer.schem"

[[source.pastes]]
path = "rom.nbt"
tick = 4
origin = { x = 10, y = 20, z = 30 }
rotation = "clockwise90"
ignore_air = true
paste_entities = false
update = true
"#,
        )
        .unwrap();

        let paste = &scenario.source.pastes[0];
        assert_eq!(paste.path, PathBuf::from("rom.nbt"));
        assert_eq!(paste.tick, Some(GameTick(4)));
        assert_eq!(paste.origin, BlockPos::new(10, 20, 30));
        assert_eq!(paste.rotation, Rotation::Clockwise90);
        assert!(paste.ignore_air);
        assert!(!paste.paste_entities);
        assert!(paste.update);
    }

    #[test]
    fn replay_camera_parses_manual_pose_and_view_distance() {
        let scenario = toml::from_str::<Scenario>(
            r#"
version = "26.1.2"
mode = "default"

[replay.camera]
view_distance = 12
position = [1.25, 2.5, 3.75]
yaw = -45.0
pitch = 30.0

[source]
path = "machine.nbt"
"#,
        )
        .unwrap();
        let camera = scenario.replay.unwrap().camera;

        assert_eq!(camera.view_distance, 12);
        assert_eq!(camera.position, Some([1.25, 2.5, 3.75]));
        assert_eq!(camera.yaw, Some(-45.0));
        assert_eq!(camera.pitch, Some(30.0));
    }
}
