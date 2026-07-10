use std::collections::{BTreeMap, HashMap, HashSet};
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum BlockBehavior {
    Air,
    Static,
    Wire,
    Lever,
    Button {
        wooden: bool,
    },
    RedstoneBlock,
    Torch {
        wall: bool,
    },
    Repeater,
    Comparator,
    Observer,
    Piston {
        sticky: bool,
    },
    Lamp,
    CopperBulb,
    PoweredConsumer,
    Door,
    PoweredRail,
    NoteBlock,
    Bell,
    Target,
    PressurePlate {
        max_weight: Option<u32>,
        detects_items: bool,
    },
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateDefinition {
    pub id: BlockStateId,
    pub kind: BlockKindId,
    pub name: Arc<str>,
    pub properties: Arc<BTreeMap<String, String>>,
    pub behavior: BlockBehavior,
    pub redstone_conductor: bool,
    pub sturdy_faces: [bool; 6],
    pub push_reaction: PushReaction,
    pub has_block_entity: bool,
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

    pub fn sturdy(&self, direction: Direction) -> bool {
        self.sturdy_faces[match direction {
            Direction::West => 0,
            Direction::East => 1,
            Direction::Down => 2,
            Direction::Up => 3,
            Direction::North => 4,
            Direction::South => 5,
        }]
    }
}

#[derive(Clone, Debug)]
pub struct Java26Registry {
    states: Vec<Option<StateDefinition>>,
    catalog: Arc<OfficialStateCatalog>,
    kinds_by_name: HashMap<String, BlockKindId>,
    names_by_kind: Vec<String>,
    property_transitions: HashMap<BlockStateId, HashMap<String, HashMap<String, BlockStateId>>>,
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
            property_transitions: HashMap::new(),
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
        value: impl AsRef<str>,
    ) -> Result<BlockStateId, StateResolveError> {
        let value = value.as_ref();
        if let Some(next) = self
            .property_transitions
            .get(&state)
            .and_then(|properties| properties.get(name))
            .and_then(|values| values.get(value))
        {
            return Ok(*next);
        }
        let definition = self
            .state(state)
            .cloned()
            .ok_or(StateResolveError::UnknownState(state))?;
        let mut properties = definition.properties.as_ref().clone();
        properties.insert(name.to_owned(), value.to_owned());
        let next = self.resolve_state(definition.name.as_ref(), &properties)?;
        self.property_transitions
            .entry(state)
            .or_default()
            .entry(name.to_owned())
            .or_default()
            .insert(value.to_owned(), next);
        Ok(next)
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
        let id = *self.catalog.states_by_key.get(&key).ok_or_else(|| {
            StateResolveError::UnknownCombination {
                name: name.to_owned(),
                properties: properties.clone(),
            }
        })?;
        if self.state(id).is_some() {
            return Ok(id);
        }

        let has_block_entity = self.catalog.blocks_with_entities.contains(name);
        let kind = self.intern_kind(name);
        let traits = classify(name, properties);
        self.states[id.0 as usize] = Some(StateDefinition {
            id,
            kind,
            name: Arc::from(name),
            properties: Arc::new(properties.clone()),
            behavior: traits.behavior,
            redstone_conductor: traits.redstone_conductor,
            sturdy_faces: traits.sturdy_faces,
            push_reaction: traits.push_reaction,
            has_block_entity,
            supported: traits.supported,
        });
        Ok(id)
    }
}

#[derive(Debug)]
struct OfficialStateCatalog {
    states_by_key: HashMap<String, BlockStateId>,
    blocks_with_entities: HashSet<String>,
    max_state_id: u32,
}

#[derive(Deserialize)]
struct BlockReportEntry {
    definition: BlockReportDefinition,
    states: Vec<BlockReportState>,
}

#[derive(Deserialize)]
struct BlockReportDefinition {
    #[serde(rename = "type")]
    block_type: String,
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
            let mut blocks_with_entities = HashSet::new();
            let mut max_state_id = 0;
            for (name, entry) in report {
                if block_definition_has_entity(&entry.definition.block_type) {
                    blocks_with_entities.insert(name.clone());
                }
                for state in entry.states {
                    max_state_id = max_state_id.max(state.id);
                    let old = states_by_key
                        .insert(state_key(&name, &state.properties), BlockStateId(state.id));
                    assert!(old.is_none(), "官方方块状态键重复: {name}");
                }
            }
            Arc::new(OfficialStateCatalog {
                states_by_key,
                blocks_with_entities,
                max_state_id,
            })
        })
        .clone()
}

fn block_definition_has_entity(block_type: &str) -> bool {
    matches!(
        block_type.strip_prefix("minecraft:").unwrap_or(block_type),
        "banner"
            | "barrel"
            | "beacon"
            | "bed"
            | "beehive"
            | "bell"
            | "blast_furnace"
            | "brewing_stand"
            | "brushable"
            | "calibrated_sculk_sensor"
            | "campfire"
            | "ceiling_hanging_sign"
            | "chest"
            | "chiseled_book_shelf"
            | "command"
            | "comparator"
            | "conduit"
            | "copper_chest"
            | "copper_golem_statue"
            | "crafter"
            | "creaking_heart"
            | "daylight_detector"
            | "decorated_pot"
            | "dispenser"
            | "dropper"
            | "enchantment_table"
            | "end_gateway"
            | "end_portal"
            | "ender_chest"
            | "furnace"
            | "hopper"
            | "jigsaw"
            | "jukebox"
            | "lectern"
            | "moving_piston"
            | "piglinwallskull"
            | "player_head"
            | "player_wall_head"
            | "sculk_catalyst"
            | "sculk_sensor"
            | "sculk_shrieker"
            | "shelf"
            | "shulker_box"
            | "skull"
            | "smoker"
            | "spawner"
            | "standing_sign"
            | "structure"
            | "test"
            | "test_instance"
            | "trapped_chest"
            | "trial_spawner"
            | "vault"
            | "wall_banner"
            | "wall_hanging_sign"
            | "wall_sign"
            | "wall_skull"
            | "weathering_copper_chest"
            | "weathering_copper_golem_statue"
            | "wither_skull"
            | "wither_wall_skull"
    )
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
            detects_items: !matches!(
                value,
                "stone_pressure_plate" | "polished_blackstone_pressure_plate"
            ),
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
        value if value.ends_with("_door") && !value.ends_with("_trapdoor") => BlockBehavior::Door,
        value if value.ends_with("_trapdoor") || value.ends_with("_fence_gate") => {
            BlockBehavior::PoweredConsumer
        }
        "powered_rail" | "activator_rail" => BlockBehavior::PoweredRail,
        "note_block" => BlockBehavior::NoteBlock,
        "bell" => BlockBehavior::Bell,
        _ => BlockBehavior::Static,
    };

    let full_slab = path.ends_with("_slab")
        && properties
            .get("type")
            .is_some_and(|slab_type| slab_type == "double");
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
            | BlockBehavior::Door
            | BlockBehavior::PoweredRail
            | BlockBehavior::NoteBlock
            | BlockBehavior::Bell
            | BlockBehavior::PressurePlate { .. }
            | BlockBehavior::Tripwire
            | BlockBehavior::TripwireHook
            | BlockBehavior::DetectorRail
            | BlockBehavior::DaylightDetector
            | BlockBehavior::Lectern
            | BlockBehavior::TrappedChest
            | BlockBehavior::UnsupportedActive
    ) || path.contains("glass")
        || (path.ends_with("_slab") && !full_slab)
        || path.ends_with("_stairs")
        || path.ends_with("_fence")
        || path.ends_with("_wall")
        || path.ends_with("_carpet")
        || path.ends_with("_rail")
        || path.ends_with("_sign");

    let redstone_conductor = !non_solid
        && !matches!(
            path,
            "redstone_block"
                | "honey_block"
                | "piston"
                | "sticky_piston"
                | "moving_piston"
                | "piston_head"
                | "hopper"
        );
    let sturdy = (!non_solid
        || path == "hopper"
        || matches!(behavior, BlockBehavior::Observer | BlockBehavior::NoteBlock))
        && !matches!(path, "moving_piston" | "piston_head");
    let extended_piston = matches!(path, "piston" | "sticky_piston")
        && properties
            .get("extended")
            .is_some_and(|value| value == "true");
    let sturdy_faces = if extended_piston {
        let mut faces = [false; 6];
        let back_face = match properties.get("facing").map(String::as_str) {
            Some("west") => 1,
            Some("east") => 0,
            Some("down") => 3,
            Some("up") => 2,
            Some("north") => 5,
            Some("south") => 4,
            _ => 1,
        };
        faces[back_face] = true;
        faces
    } else {
        [sturdy; 6]
    };
    let push_reaction = piston_push_reaction(path, extended_piston);
    let supported = !matches!(behavior, BlockBehavior::UnsupportedActive);

    BlockTraits {
        behavior,
        redstone_conductor,
        sturdy_faces,
        push_reaction,
        supported,
    }
}

fn piston_push_reaction(path: &str, extended_piston: bool) -> PushReaction {
    if extended_piston
        || matches!(
            path,
            "bedrock"
                | "obsidian"
                | "crying_obsidian"
                | "respawn_anchor"
                | "reinforced_deepslate"
                | "end_portal_frame"
                | "moving_piston"
                | "piston_head"
                | "barrier"
                | "light"
                | "nether_portal"
                | "end_portal"
                | "end_gateway"
                | "anvil"
                | "chipped_anvil"
                | "damaged_anvil"
                | "grindstone"
                | "lodestone"
        )
    {
        PushReaction::Block
    } else if path.ends_with("_glazed_terracotta") {
        PushReaction::PushOnly
    } else if path.ends_with("_torch")
        || path == "redstone_wire"
        || path.ends_with("_button")
        || path.ends_with("_pressure_plate")
        || path.ends_with("_door") && !path.ends_with("_trapdoor")
        || matches!(
            path,
            "lever"
                | "repeater"
                | "comparator"
                | "tripwire"
                | "tripwire_hook"
                | "structure_void"
                | "decorated_pot"
        )
    {
        PushReaction::Destroy
    } else {
        PushReaction::Normal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn piston_push_reactions_match_vanilla_categories() {
        for path in [
            "repeater",
            "comparator",
            "stone_pressure_plate",
            "tripwire_hook",
            "oak_door",
            "structure_void",
            "decorated_pot",
        ] {
            assert_eq!(
                piston_push_reaction(path, false),
                PushReaction::Destroy,
                "{path}"
            );
        }
        for path in [
            "barrier",
            "light",
            "nether_portal",
            "end_portal",
            "end_gateway",
            "anvil",
            "grindstone",
            "lodestone",
        ] {
            assert_eq!(
                piston_push_reaction(path, false),
                PushReaction::Block,
                "{path}"
            );
        }
        assert_eq!(
            piston_push_reaction("white_glazed_terracotta", false),
            PushReaction::PushOnly
        );
        assert_eq!(
            piston_push_reaction("sticky_piston", true),
            PushReaction::Block
        );
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
