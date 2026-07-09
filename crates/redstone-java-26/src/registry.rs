use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use redstone_core::{BlockKindId, BlockStateId, Direction};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub trait StateResolver {
    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, StateResolveError>;
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum PushReaction {
    Normal,
    Block,
    Destroy,
    PushOnly,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum BlockBehavior {
    Air,
    Static,
    Wire,
    Lever,
    Button { wooden: bool },
    RedstoneBlock,
    Torch { wall: bool },
    Repeater,
    Comparator,
    Observer,
    Piston { sticky: bool },
    Lamp,
    CopperBulb,
    PoweredConsumer,
    Target,
    PressurePlate { max_weight: Option<u32>, detects_items: bool },
    Tripwire,
    TripwireHook,
    DetectorRail,
    DaylightDetector,
    Lectern,
    TrappedChest,
    Hopper,
    Dropper,
    Dispenser,
    Crafter,
    Tnt,
    UnsupportedActive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StateDefinition {
    pub id: BlockStateId,
    pub kind: BlockKindId,
    pub name: String,
    pub properties: BTreeMap<String, String>,
    pub behavior: BlockBehavior,
    pub redstone_conductor: bool,
    pub sturdy_faces: [bool; 6],
    pub push_reaction: PushReaction,
    pub supported: bool,
}

impl StateDefinition {
    pub fn property(&self, name: &str) -> Option<&str> {
        self.properties.get(name).map(String::as_str)
    }

    pub fn bool_property(&self, name: &str) -> bool {
        self.property(name) == Some("true")
    }

    pub fn int_property(&self, name: &str) -> Option<i32> {
        self.property(name)?.parse().ok()
    }

    pub fn direction_property(&self, name: &str) -> Option<Direction> {
        match self.property(name)? {
            "west" => Some(Direction::West),
            "east" => Some(Direction::East),
            "down" => Some(Direction::Down),
            "up" => Some(Direction::Up),
            "north" => Some(Direction::North),
            "south" => Some(Direction::South),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Java26Registry {
    states: Vec<Option<StateDefinition>>,
    catalog: Arc<OfficialStateCatalog>,
    kinds_by_name: HashMap<String, BlockKindId>,
    names_by_kind: Vec<String>,
}

impl Default for Java26Registry {
    fn default() -> Self {
        Self::new()
    }
}

impl Java26Registry {
    pub fn new() -> Self {
        let catalog = official_catalog();
        let mut registry = Self {
            states: vec![None; catalog.max_state_id as usize + 1],
            catalog,
            kinds_by_name: HashMap::new(),
            names_by_kind: Vec::new(),
        };
        registry
            .resolve_state("minecraft:air", &BTreeMap::new())
            .expect("air state must be valid");
        registry
    }

    pub const fn air_state(&self) -> BlockStateId {
        BlockStateId(0)
    }

    pub fn state(&self, id: BlockStateId) -> Option<&StateDefinition> {
        self.states.get(id.0 as usize)?.as_ref()
    }

    pub fn states(&self) -> impl Iterator<Item = &StateDefinition> {
        self.states.iter().filter_map(Option::as_ref)
    }

    pub fn kind_name(&self, kind: BlockKindId) -> Option<&str> {
        self.names_by_kind.get(kind.0 as usize).map(String::as_str)
    }

    pub fn with_property(
        &mut self,
        state: BlockStateId,
        name: &str,
        value: impl Into<String>,
    ) -> Result<BlockStateId, StateResolveError> {
        let definition = self
            .state(state)
            .cloned()
            .ok_or(StateResolveError::UnknownState(state))?;
        let mut properties = definition.properties;
        properties.insert(name.to_owned(), value.into());
        self.resolve_state(&definition.name, &properties)
    }

    pub fn state_by_name(
        &mut self,
        name: &str,
        properties: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Result<BlockStateId, StateResolveError> {
        let properties = properties
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect::<BTreeMap<_, _>>();
        self.resolve_state(name, &properties)
    }

    fn intern_kind(&mut self, name: &str) -> BlockKindId {
        if let Some(kind) = self.kinds_by_name.get(name) {
            return *kind;
        }
        let kind = BlockKindId(self.names_by_kind.len() as u32);
        self.names_by_kind.push(name.to_owned());
        self.kinds_by_name.insert(name.to_owned(), kind);
        kind
    }
}

impl StateResolver for Java26Registry {
    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, StateResolveError> {
        if !name.contains(':') {
            return Err(StateResolveError::InvalidName(name.to_owned()));
        }
        let key = state_key(name, properties);
        let id = *self
            .catalog
            .states_by_key
            .get(&key)
            .ok_or_else(|| StateResolveError::UnknownCombination {
                name: name.to_owned(),
                properties: properties.clone(),
            })?;
        if self.state(id).is_some() {
            return Ok(id);
        }

        let kind = self.intern_kind(name);
        let traits = classify(name, properties);
        self.states[id.0 as usize] = Some(StateDefinition {
            id,
            kind,
            name: name.to_owned(),
            properties: properties.clone(),
            behavior: traits.behavior,
            redstone_conductor: traits.redstone_conductor,
            sturdy_faces: traits.sturdy_faces,
            push_reaction: traits.push_reaction,
            supported: traits.supported,
        });
        Ok(id)
    }
}

#[derive(Debug)]
struct OfficialStateCatalog {
    states_by_key: HashMap<String, BlockStateId>,
    max_state_id: u32,
}

#[derive(Deserialize)]
struct BlockReportEntry {
    states: Vec<BlockReportState>,
}

#[derive(Deserialize)]
struct BlockReportState {
    id: u32,
    #[serde(default)]
    properties: BTreeMap<String, String>,
}

fn official_catalog() -> Arc<OfficialStateCatalog> {
    static CATALOG: OnceLock<Arc<OfficialStateCatalog>> = OnceLock::new();
    CATALOG
        .get_or_init(|| {
            let report = serde_json::from_str::<BTreeMap<String, BlockReportEntry>>(include_str!(
                "../data/26.1.2/reports/blocks.json"
            ))
            .expect("官方 26.1.2 blocks.json 必须可解析");
            let mut states_by_key = HashMap::new();
            let mut max_state_id = 0;
            for (name, entry) in report {
                for state in entry.states {
                    max_state_id = max_state_id.max(state.id);
                    let old = states_by_key.insert(
                        state_key(&name, &state.properties),
                        BlockStateId(state.id),
                    );
                    assert!(old.is_none(), "官方方块状态键重复: {name}");
                }
            }
            Arc::new(OfficialStateCatalog {
                states_by_key,
                max_state_id,
            })
        })
        .clone()
}

fn state_key(name: &str, properties: &BTreeMap<String, String>) -> String {
    let mut key = name.to_owned();
    for (name, value) in properties {
        key.push('|');
        key.push_str(name);
        key.push('=');
        key.push_str(value);
    }
    key
}

struct BlockTraits {
    behavior: BlockBehavior,
    redstone_conductor: bool,
    sturdy_faces: [bool; 6],
    push_reaction: PushReaction,
    supported: bool,
}

fn classify(name: &str, properties: &BTreeMap<String, String>) -> BlockTraits {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    let behavior = match path {
        "air" | "cave_air" | "void_air" => BlockBehavior::Air,
        "redstone_wire" => BlockBehavior::Wire,
        "lever" => BlockBehavior::Lever,
        value if value.ends_with("_button") => BlockBehavior::Button {
            wooden: !matches!(value, "stone_button" | "polished_blackstone_button"),
        },
        "redstone_block" => BlockBehavior::RedstoneBlock,
        "redstone_torch" => BlockBehavior::Torch { wall: false },
        "redstone_wall_torch" => BlockBehavior::Torch { wall: true },
        "repeater" => BlockBehavior::Repeater,
        "comparator" => BlockBehavior::Comparator,
        "observer" => BlockBehavior::Observer,
        "piston" => BlockBehavior::Piston { sticky: false },
        "sticky_piston" => BlockBehavior::Piston { sticky: true },
        "redstone_lamp" => BlockBehavior::Lamp,
        value if value.ends_with("_bulb") => BlockBehavior::CopperBulb,
        "target" => BlockBehavior::Target,
        "light_weighted_pressure_plate" => BlockBehavior::PressurePlate {
            max_weight: Some(15),
            detects_items: true,
        },
        "heavy_weighted_pressure_plate" => BlockBehavior::PressurePlate {
            max_weight: Some(150),
            detects_items: true,
        },
        value if value.ends_with("_pressure_plate") => BlockBehavior::PressurePlate {
            max_weight: None,
            detects_items: !matches!(value, "stone_pressure_plate" | "polished_blackstone_pressure_plate"),
        },
        "tripwire" => BlockBehavior::Tripwire,
        "tripwire_hook" => BlockBehavior::TripwireHook,
        "detector_rail" => BlockBehavior::DetectorRail,
        "daylight_detector" => BlockBehavior::DaylightDetector,
        "lectern" => BlockBehavior::Lectern,
        "trapped_chest" => BlockBehavior::TrappedChest,
        "hopper" => BlockBehavior::Hopper,
        "dropper" => BlockBehavior::Dropper,
        "dispenser" => BlockBehavior::Dispenser,
        "crafter" => BlockBehavior::Crafter,
        "tnt" => BlockBehavior::Tnt,
        value
            if value.ends_with("_door")
                || value.ends_with("_trapdoor")
                || value.ends_with("_fence_gate") => BlockBehavior::PoweredConsumer,
        "powered_rail" | "activator_rail" | "note_block" | "bell" => {
            BlockBehavior::PoweredConsumer
        }
        _ => BlockBehavior::Static,
    };

    let non_solid = matches!(
        behavior,
        BlockBehavior::Air
            | BlockBehavior::Wire
            | BlockBehavior::Lever
            | BlockBehavior::Button { .. }
            | BlockBehavior::Torch { .. }
            | BlockBehavior::Repeater
            | BlockBehavior::Comparator
            | BlockBehavior::Observer
            | BlockBehavior::PoweredConsumer
            | BlockBehavior::PressurePlate { .. }
            | BlockBehavior::Tripwire
            | BlockBehavior::TripwireHook
            | BlockBehavior::DetectorRail
            | BlockBehavior::DaylightDetector
            | BlockBehavior::Lectern
            | BlockBehavior::TrappedChest
            | BlockBehavior::UnsupportedActive
    ) || path.contains("glass")
        || path.ends_with("_slab")
        || path.ends_with("_stairs")
        || path.ends_with("_fence")
        || path.ends_with("_wall")
        || path.ends_with("_carpet")
        || path.ends_with("_rail")
        || path.ends_with("_sign");

    let redstone_conductor = !non_solid
        && !matches!(
            path,
            "slime_block" | "honey_block" | "moving_piston" | "piston_head"
        );
    let sturdy = !non_solid || path == "hopper";
    let push_reaction = if matches!(
        path,
        "bedrock"
            | "obsidian"
            | "crying_obsidian"
            | "respawn_anchor"
            | "reinforced_deepslate"
            | "end_portal_frame"
            | "moving_piston"
            | "piston_head"
    ) {
        PushReaction::Block
    } else if path.ends_with("_torch")
        || path == "redstone_wire"
        || path.ends_with("_button")
        || path == "lever"
        || path == "tripwire"
    {
        PushReaction::Destroy
    } else {
        PushReaction::Normal
    };
    let supported = !matches!(behavior, BlockBehavior::UnsupportedActive);

    let _ = properties;
    BlockTraits {
        behavior,
        redstone_conductor,
        sturdy_faces: [sturdy; 6],
        push_reaction,
        supported,
    }
}

#[derive(Debug, Error)]
pub enum StateResolveError {
    #[error("无效方块名称: {0}")]
    InvalidName(String),
    #[error("未知方块状态: {0:?}")]
    UnknownState(BlockStateId),
    #[error("26.1.2 不存在方块状态: {name} {properties:?}")]
    UnknownCombination {
        name: String,
        properties: BTreeMap<String, String>,
    },
}
