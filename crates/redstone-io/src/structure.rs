use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};

use fastnbt::Value;
use flate2::read::GzDecoder;
use redstone_core::{BlockEntityData, BlockPos, BlockStateId, Direction, EntityData, SparseWorld};
use serde::Serialize;
use thiserror::Error;
use tracing::info;

mod litematic;
mod litematic_writer;
mod sponge;
mod sponge_writer;
mod vanilla;
mod vanilla_writer;
mod world;
mod writer;

pub use vanilla_writer::VanillaWriteError;
pub use writer::{
    StructureState, StructureWriteError, StructureWriteOptions, StructureWriter,
};

pub trait StructureStateResolver {
    type Error: std::error::Error + Send + Sync + 'static;

    fn resolve_state(
        &mut self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BlockStateId, Self::Error>;

    fn complete_state_properties(
        &mut self,
        _name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<BTreeMap<String, String>, Self::Error> {
        Ok(properties.clone())
    }

    fn air_state(&self) -> BlockStateId;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StructureFormat {
    Litematic,
    SpongeSchematic,
    VanillaStructure,
    MinecraftWorld,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct StructureRegion {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl StructureRegion {
    pub fn new(first: BlockPos, second: BlockPos) -> Self {
        Self {
            min: BlockPos::new(
                first.x.min(second.x),
                first.y.min(second.y),
                first.z.min(second.z),
            ),
            max: BlockPos::new(
                first.x.max(second.x),
                first.y.max(second.y),
                first.z.max(second.z),
            ),
        }
    }

    pub fn contains(self, pos: BlockPos) -> bool {
        (self.min.x..=self.max.x).contains(&pos.x)
            && (self.min.y..=self.max.y).contains(&pos.y)
            && (self.min.z..=self.max.z).contains(&pos.z)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StructureLoadOptions {
    pub transform: StructureTransform,
    pub region: Option<StructureRegion>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    None,
    Clockwise90,
    Clockwise180,
    Counterclockwise90,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mirror {
    #[default]
    None,
    LeftRight,
    FrontBack,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StructureTransform {
    pub origin: BlockPos,
    pub rotation: Rotation,
    pub mirror: Mirror,
}

impl StructureTransform {
    pub fn apply(self, pos: BlockPos) -> BlockPos {
        let mirrored = match self.mirror {
            Mirror::None => pos,
            Mirror::LeftRight => BlockPos::new(pos.x, pos.y, -pos.z),
            Mirror::FrontBack => BlockPos::new(-pos.x, pos.y, pos.z),
        };
        let rotated = match self.rotation {
            Rotation::None => mirrored,
            Rotation::Clockwise90 => BlockPos::new(-mirrored.z, mirrored.y, mirrored.x),
            Rotation::Clockwise180 => BlockPos::new(-mirrored.x, mirrored.y, -mirrored.z),
            Rotation::Counterclockwise90 => BlockPos::new(mirrored.z, mirrored.y, -mirrored.x),
        };
        self.origin.offset(rotated.x, rotated.y, rotated.z)
    }

    pub fn apply_point(self, point: [f64; 3]) -> [f64; 3] {
        let mirrored = match self.mirror {
            Mirror::None => point,
            Mirror::LeftRight => [point[0], point[1], -point[2]],
            Mirror::FrontBack => [-point[0], point[1], point[2]],
        };
        let rotated = match self.rotation {
            Rotation::None => mirrored,
            Rotation::Clockwise90 => [-mirrored[2], mirrored[1], mirrored[0]],
            Rotation::Clockwise180 => [-mirrored[0], mirrored[1], -mirrored[2]],
            Rotation::Counterclockwise90 => [mirrored[2], mirrored[1], -mirrored[0]],
        };
        [
            rotated[0] + self.origin.x as f64,
            rotated[1] + self.origin.y as f64,
            rotated[2] + self.origin.z as f64,
        ]
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct LoadedStructure {
    pub world: SparseWorld,
    pub min: BlockPos,
    pub max: BlockPos,
    pub region_min: BlockPos,
    pub region_max: BlockPos,
    pub format: String,
    pub data_version: Option<i32>,
    pub block_counts: BTreeMap<String, usize>,
    pub block_entity_nbt: BTreeMap<BlockPos, BTreeMap<String, serde_json::Value>>,
}

pub struct StructureLoader;

impl StructureLoader {
    pub fn detect(path: &Path) -> Result<StructureFormat, StructureError> {
        if path.is_dir() {
            return Ok(StructureFormat::MinecraftWorld);
        }
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("litematic") => Ok(StructureFormat::Litematic),
            Some("schem") => Ok(StructureFormat::SpongeSchematic),
            Some("nbt" | "structure") => Ok(StructureFormat::VanillaStructure),
            _ => Err(StructureError::UnknownFormat(path.to_path_buf())),
        }
    }

    pub fn load<R: StructureStateResolver>(
        path: impl AsRef<Path>,
        origin: BlockPos,
        resolver: &mut R,
    ) -> Result<LoadedStructure, StructureError> {
        Self::load_with_options(
            path,
            StructureLoadOptions {
                transform: StructureTransform {
                    origin,
                    ..StructureTransform::default()
                },
                region: None,
            },
            resolver,
        )
    }

    pub fn load_transformed<R: StructureStateResolver>(
        path: impl AsRef<Path>,
        transform: StructureTransform,
        resolver: &mut R,
    ) -> Result<LoadedStructure, StructureError> {
        Self::load_with_options(
            path,
            StructureLoadOptions {
                transform,
                region: None,
            },
            resolver,
        )
    }

    pub fn load_with_options<R: StructureStateResolver>(
        path: impl AsRef<Path>,
        options: StructureLoadOptions,
        resolver: &mut R,
    ) -> Result<LoadedStructure, StructureError> {
        let path = path.as_ref();
        let format = Self::detect(path)?;
        if format == StructureFormat::MinecraftWorld {
            return world::load(path, options, resolver);
        }
        if options.region.is_some() {
            return Err(StructureError::RegionOnlyForWorld);
        }
        let root = read_nbt(path)?;
        info!(path = %path.display(), ?format, "读取结构文件");
        match format {
            StructureFormat::VanillaStructure => {
                vanilla::load(root, options.transform, resolver)
            }
            StructureFormat::Litematic => litematic::load(root, options.transform, resolver),
            StructureFormat::SpongeSchematic => {
                sponge::load(root, options.transform, resolver)
            }
            StructureFormat::MinecraftWorld => unreachable!(),
        }
    }
}

fn read_nbt(path: &Path) -> Result<HashMap<String, Value>, StructureError> {
    let bytes = std::fs::read(path)?;
    let mut decoded = Vec::new();
    if bytes.starts_with(&[0x1f, 0x8b]) {
        GzDecoder::new(bytes.as_slice()).read_to_end(&mut decoded)?;
    } else {
        decoded = bytes;
    }
    fastnbt::from_bytes(&decoded).map_err(StructureError::Nbt)
}

pub(super) fn resolve<R: StructureStateResolver>(
    resolver: &mut R,
    name: &str,
    properties: &BTreeMap<String, String>,
) -> Result<BlockStateId, StructureError> {
    resolver
        .resolve_state(name, properties)
        .map_err(|error| StructureError::Resolve(error.to_string()))
}

pub(super) fn complete_properties<R: StructureStateResolver>(
    resolver: &mut R,
    name: &str,
    properties: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>, StructureError> {
    resolver
        .complete_state_properties(name, properties)
        .map_err(|error| StructureError::Resolve(error.to_string()))
}

pub(super) fn update_bounds(
    min: &mut Option<BlockPos>,
    max: &mut Option<BlockPos>,
    pos: BlockPos,
) {
    *min = Some(min.map_or(pos, |old| {
        BlockPos::new(old.x.min(pos.x), old.y.min(pos.y), old.z.min(pos.z))
    }));
    *max = Some(max.map_or(pos, |old| {
        BlockPos::new(old.x.max(pos.x), old.y.max(pos.y), old.z.max(pos.z))
    }));
}

pub(super) fn transformed_bounds(
    transform: StructureTransform,
    min: BlockPos,
    max: BlockPos,
) -> (BlockPos, BlockPos) {
    let mut transformed_min = None;
    let mut transformed_max = None;
    for x in [min.x, max.x] {
        for y in [min.y, max.y] {
            for z in [min.z, max.z] {
                update_bounds(
                    &mut transformed_min,
                    &mut transformed_max,
                    transform.apply(BlockPos::new(x, y, z)),
                );
            }
        }
    }
    (
        transformed_min.unwrap_or(transform.origin),
        transformed_max.unwrap_or(transform.origin),
    )
}

pub(super) fn nbt_to_block_entity(nbt: &HashMap<String, Value>) -> BlockEntityData {
    let kind = nbt
        .get("id")
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "minecraft:unknown".to_owned());
    let mut fields = nbt
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "id" | "x" | "y" | "z"))
        .map(|(key, value)| (key.clone(), nbt_value_to_json(value)))
        .collect::<BTreeMap<_, _>>();
    normalize_inventory_fields(&kind, nbt, &mut fields);
    BlockEntityData { kind, fields }
}

pub(super) fn nbt_compound_to_json(
    nbt: &HashMap<String, Value>,
) -> BTreeMap<String, serde_json::Value> {
    nbt.iter()
        .map(|(key, value)| (key.clone(), nbt_value_to_json(value)))
        .collect()
}

pub(super) fn entity_from_nbt(
    entity: &HashMap<String, Value>,
) -> Result<Option<EntityData>, StructureError> {
    let nbt = match entity.get("nbt") {
        Some(Value::Compound(nbt)) => nbt,
        _ => entity,
    };
    let kind = nbt
        .get("id")
        .or_else(|| entity.get("id"))
        .and_then(|value| match value {
            Value::String(value) => Some(value.clone()),
            _ => None,
        });
    let Some(kind) = kind else {
        return Ok(None);
    };
    let position = match entity.get("pos").or_else(|| nbt.get("Pos")) {
        Some(Value::List(values)) if values.len() == 3 => [
            value_f64(&values[0]).ok_or(StructureError::InvalidType("entity x"))?,
            value_f64(&values[1]).ok_or(StructureError::InvalidType("entity y"))?,
            value_f64(&values[2]).ok_or(StructureError::InvalidType("entity z"))?,
        ],
        _ => [0.0, 0.0, 0.0],
    };
    let mut fields = nbt
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "id" | "Pos"))
        .map(|(key, value)| (key.clone(), nbt_value_to_json(value)))
        .collect::<BTreeMap<_, _>>();
    normalize_inventory_fields(&kind, nbt, &mut fields);
    normalize_item_entity_fields(&kind, nbt, &mut fields);
    normalize_item_frame_fields(&kind, nbt, &mut fields);
    Ok(Some(EntityData {
        kind,
        position,
        fields,
    }))
}

fn normalize_inventory_fields(
    kind: &str,
    nbt: &HashMap<String, Value>,
    fields: &mut BTreeMap<String, serde_json::Value>,
) {
    let Some(Value::List(items)) = nbt.get("Items").or_else(|| nbt.get("items")) else {
        normalize_cooldown(nbt, fields);
        return;
    };
    let mut inventory = items
        .iter()
        .filter_map(|item| {
            let Value::Compound(item) = item else {
                return None;
            };
            let slot = item
                .get("Slot")
                .or_else(|| item.get("slot"))
                .and_then(value_i32)?;
            let item_id = item.get("id").and_then(|value| match value {
                Value::String(value) => Some(value.clone()),
                _ => None,
            })?;
            let count = item
                .get("count")
                .or_else(|| item.get("Count"))
                .and_then(value_i32)?;
            let components = item.get("components").map(nbt_value_to_json);
            (count > 0).then_some((slot, item_id, count, components))
        })
        .collect::<Vec<_>>();
    inventory.sort_by_key(|(slot, _, _, _)| *slot);
    let item_count = inventory
        .iter()
        .map(|(_, _, count, _)| i64::from(*count))
        .sum::<i64>();
    let first_item = inventory.first().map(|(_, item_id, _, _)| item_id.clone());
    let inventory = inventory
        .into_iter()
        .map(|(slot, item_id, count, components)| {
            let mut item = serde_json::Map::from_iter([
                ("slot".to_owned(), serde_json::Value::from(slot)),
                ("item_id".to_owned(), serde_json::Value::String(item_id)),
                ("count".to_owned(), serde_json::Value::from(count)),
            ]);
            if let Some(components) = components {
                item.insert("components".to_owned(), components);
            }
            serde_json::Value::Object(item)
        })
        .collect::<Vec<_>>();
    let slot_count = container_slot_count(kind, fields, &inventory);
    fields.insert("inventory".to_owned(), serde_json::Value::Array(inventory));
    fields.insert("item_count".to_owned(), serde_json::Value::from(item_count));
    fields.insert("slot_count".to_owned(), serde_json::Value::from(slot_count));
    fields.insert(
        "capacity".to_owned(),
        serde_json::Value::from(i64::from(slot_count) * 64),
    );
    if let Some(item_id) = first_item {
        fields.insert("item_id".to_owned(), serde_json::Value::String(item_id));
    }
    normalize_cooldown(nbt, fields);
}

fn normalize_cooldown(
    nbt: &HashMap<String, Value>,
    fields: &mut BTreeMap<String, serde_json::Value>,
) {
    if let Some(cooldown) = nbt
        .get("TransferCooldown")
        .or_else(|| nbt.get("transfer_cooldown"))
        .and_then(value_i32)
    {
        fields.insert("cooldown".to_owned(), serde_json::Value::from(cooldown));
    }
}

fn container_slot_count(
    kind: &str,
    fields: &BTreeMap<String, serde_json::Value>,
    inventory: &[serde_json::Value],
) -> i32 {
    let known = match kind {
        "minecraft:hopper" | "minecraft:brewing_stand" => Some(5),
        "minecraft:dispenser" | "minecraft:dropper" | "minecraft:crafter" => Some(9),
        "minecraft:furnace" | "minecraft:blast_furnace" | "minecraft:smoker" => Some(3),
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:barrel" => Some(27),
        value if value.ends_with("copper_chest") => Some(27),
        "minecraft:decorated_pot" => Some(1),
        "minecraft:chest_minecart" => Some(27),
        "minecraft:hopper_minecart" => Some(5),
        value if value.ends_with("_shulker_box") || value == "minecraft:shulker_box" => Some(27),
        _ => None,
    };
    known.unwrap_or_else(|| {
        inventory
            .iter()
            .filter_map(|entry| entry.get("slot").and_then(serde_json::Value::as_i64))
            .max()
            .map_or(1, |slot| {
                slot.saturating_add(1).clamp(1, i64::from(i32::MAX)) as i32
            })
            .max(
                fields
                    .get("Size")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(1)
                    .clamp(1, i64::from(i32::MAX)) as i32,
            )
    })
}

fn normalize_item_entity_fields(
    kind: &str,
    nbt: &HashMap<String, Value>,
    fields: &mut BTreeMap<String, serde_json::Value>,
) {
    if kind != "minecraft:item" {
        return;
    }
    let Some(Value::Compound(item)) = nbt.get("Item").or_else(|| nbt.get("item")) else {
        return;
    };
    if let Some(Value::String(item_id)) = item.get("id") {
        fields.insert(
            "item_id".to_owned(),
            serde_json::Value::String(item_id.clone()),
        );
    }
    if let Some(count) = item
        .get("count")
        .or_else(|| item.get("Count"))
        .and_then(value_i32)
    {
        fields.insert("item_count".to_owned(), serde_json::Value::from(count));
    }
    if let Some(age) = nbt
        .get("Age")
        .or_else(|| nbt.get("age"))
        .and_then(value_i32)
    {
        fields.insert("age".to_owned(), serde_json::Value::from(age));
    }
    if let Some(delay) = nbt
        .get("PickupDelay")
        .or_else(|| nbt.get("pickup_delay"))
        .and_then(value_i32)
    {
        fields.insert("pickup_delay".to_owned(), serde_json::Value::from(delay));
    }
}

fn normalize_item_frame_fields(
    kind: &str,
    nbt: &HashMap<String, Value>,
    fields: &mut BTreeMap<String, serde_json::Value>,
) {
    if !matches!(kind, "minecraft:item_frame" | "minecraft:glow_item_frame") {
        return;
    }
    if let Some(Value::Compound(item)) = nbt.get("Item").or_else(|| nbt.get("item")) {
        if let Some(Value::String(item_id)) = item.get("id") {
            fields.insert(
                "item_id".to_owned(),
                serde_json::Value::String(item_id.clone()),
            );
        }
        if let Some(count) = item
            .get("count")
            .or_else(|| item.get("Count"))
            .and_then(value_i32)
        {
            fields.insert("item_count".to_owned(), serde_json::Value::from(count));
        }
    }
    if let Some(rotation) = nbt
        .get("ItemRotation")
        .or_else(|| nbt.get("item_rotation"))
        .and_then(value_i32)
    {
        fields.insert(
            "rotation".to_owned(),
            serde_json::Value::from(rotation.rem_euclid(8)),
        );
    }
    if let Some(facing) = nbt.get("Facing").or_else(|| nbt.get("facing")) {
        let facing = match facing {
            Value::String(value) => Some(serde_json::Value::String(value.clone())),
            value => value_i32(value).map(serde_json::Value::from),
        };
        if let Some(facing) = facing {
            fields.insert("facing".to_owned(), facing);
        }
    }
}

pub(super) fn transform_entity(
    mut entity: EntityData,
    transform: StructureTransform,
    offset: BlockPos,
) -> EntityData {
    entity.position = transform.apply_point([
        entity.position[0] + offset.x as f64,
        entity.position[1] + offset.y as f64,
        entity.position[2] + offset.z as f64,
    ]);
    if let Some(facing) = entity.fields.get("facing").and_then(json_direction) {
        entity.fields.insert(
            "facing".to_owned(),
            serde_json::Value::String(
                direction_name(transform_direction(facing, transform)).to_owned(),
            ),
        );
    }
    entity
}

fn json_direction(value: &serde_json::Value) -> Option<Direction> {
    match value {
        serde_json::Value::String(value) => parse_direction(value),
        serde_json::Value::Number(value) => match value.as_i64()? {
            0 => Some(Direction::Down),
            1 => Some(Direction::Up),
            2 => Some(Direction::North),
            3 => Some(Direction::South),
            4 => Some(Direction::West),
            5 => Some(Direction::East),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn transform_properties(
    properties: BTreeMap<String, String>,
    transform: StructureTransform,
) -> BTreeMap<String, String> {
    let mut transformed = BTreeMap::new();
    for (key, value) in properties {
        if let Some(direction) = parse_horizontal_direction(&key) {
            transformed.insert(
                direction_name(transform_direction(direction, transform)).to_owned(),
                value,
            );
            continue;
        }
        let value = match key.as_str() {
            "facing" => parse_direction(&value)
                .map(|direction| {
                    direction_name(transform_direction(direction, transform)).to_owned()
                })
                .unwrap_or(value),
            "axis"
                if matches!(
                    transform.rotation,
                    Rotation::Clockwise90 | Rotation::Counterclockwise90
                ) =>
            {
                match value.as_str() {
                    "x" => "z".to_owned(),
                    "z" => "x".to_owned(),
                    _ => value,
                }
            }
            "hinge" if transform.mirror != Mirror::None => match value.as_str() {
                "left" => "right".to_owned(),
                "right" => "left".to_owned(),
                _ => value,
            },
            "shape" if transform.mirror != Mirror::None => match value.as_str() {
                "inner_left" => "inner_right".to_owned(),
                "inner_right" => "inner_left".to_owned(),
                "outer_left" => "outer_right".to_owned(),
                "outer_right" => "outer_left".to_owned(),
                _ => transform_rail_shape(&value, transform),
            },
            "shape" => transform_rail_shape(&value, transform),
            _ => value,
        };
        transformed.insert(key, value);
    }
    transformed
}

fn transform_rail_shape(value: &str, transform: StructureTransform) -> String {
    let directions = match value {
        "north_south" => Some(("north", "south", "")),
        "east_west" => Some(("east", "west", "")),
        "ascending_north" => Some(("north", "", "ascending_")),
        "ascending_south" => Some(("south", "", "ascending_")),
        "ascending_east" => Some(("east", "", "ascending_")),
        "ascending_west" => Some(("west", "", "ascending_")),
        "north_east" => Some(("north", "east", "")),
        "north_west" => Some(("north", "west", "")),
        "south_east" => Some(("south", "east", "")),
        "south_west" => Some(("south", "west", "")),
        _ => None,
    };
    let Some((first, second, prefix)) = directions else {
        return value.to_owned();
    };
    let first = transform_direction(parse_direction(first).unwrap(), transform);
    if second.is_empty() {
        return format!("{prefix}{}", direction_name(first));
    }
    let second = transform_direction(parse_direction(second).unwrap(), transform);
    canonical_pair(first, second)
}

fn canonical_pair(first: Direction, second: Direction) -> String {
    let north_south = [Direction::North, Direction::South];
    let east_west = [Direction::East, Direction::West];
    if north_south.contains(&first) && north_south.contains(&second) {
        return "north_south".to_owned();
    }
    if east_west.contains(&first) && east_west.contains(&second) {
        return "east_west".to_owned();
    }
    let north_or_south = if matches!(first, Direction::North | Direction::South) {
        first
    } else {
        second
    };
    let east_or_west = if matches!(first, Direction::East | Direction::West) {
        first
    } else {
        second
    };
    format!(
        "{}_{}",
        direction_name(north_or_south),
        direction_name(east_or_west)
    )
}

fn transform_direction(direction: Direction, transform: StructureTransform) -> Direction {
    let mirrored = match transform.mirror {
        Mirror::None => direction,
        Mirror::LeftRight => match direction {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            other => other,
        },
        Mirror::FrontBack => match direction {
            Direction::West => Direction::East,
            Direction::East => Direction::West,
            other => other,
        },
    };
    match transform.rotation {
        Rotation::None => mirrored,
        Rotation::Clockwise90 => match mirrored {
            Direction::North => Direction::East,
            Direction::East => Direction::South,
            Direction::South => Direction::West,
            Direction::West => Direction::North,
            other => other,
        },
        Rotation::Clockwise180 => match mirrored {
            Direction::North => Direction::South,
            Direction::South => Direction::North,
            Direction::West => Direction::East,
            Direction::East => Direction::West,
            other => other,
        },
        Rotation::Counterclockwise90 => match mirrored {
            Direction::North => Direction::West,
            Direction::West => Direction::South,
            Direction::South => Direction::East,
            Direction::East => Direction::North,
            other => other,
        },
    }
}

fn parse_horizontal_direction(value: &str) -> Option<Direction> {
    match parse_direction(value) {
        Some(direction) if direction.is_horizontal() => Some(direction),
        _ => None,
    }
}

fn parse_direction(value: &str) -> Option<Direction> {
    match value {
        "west" => Some(Direction::West),
        "east" => Some(Direction::East),
        "down" => Some(Direction::Down),
        "up" => Some(Direction::Up),
        "north" => Some(Direction::North),
        "south" => Some(Direction::South),
        _ => None,
    }
}

fn direction_name(direction: Direction) -> &'static str {
    match direction {
        Direction::West => "west",
        Direction::East => "east",
        Direction::Down => "down",
        Direction::Up => "up",
        Direction::North => "north",
        Direction::South => "south",
    }
}

fn nbt_value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Byte(value) => serde_json::Value::from(*value),
        Value::Short(value) => serde_json::Value::from(*value),
        Value::Int(value) => serde_json::Value::from(*value),
        Value::Long(value) => serde_json::Value::from(*value),
        Value::Float(value) => serde_json::Value::from(*value),
        Value::Double(value) => serde_json::Value::from(*value),
        Value::String(value) => serde_json::Value::from(value.clone()),
        Value::ByteArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|value| serde_json::Value::from(*value))
                .collect(),
        ),
        Value::IntArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|value| serde_json::Value::from(*value))
                .collect(),
        ),
        Value::LongArray(value) => serde_json::Value::Array(
            value
                .iter()
                .map(|value| serde_json::Value::from(*value))
                .collect(),
        ),
        Value::List(value) => {
            serde_json::Value::Array(value.iter().map(nbt_value_to_json).collect())
        }
        Value::Compound(value) => serde_json::Value::Object(
            value
                .iter()
                .map(|(key, value)| (key.clone(), nbt_value_to_json(value)))
                .collect(),
        ),
    }
}

pub(super) fn value_i32(value: &Value) -> Option<i32> {
    match value {
        Value::Byte(value) => Some(*value as i32),
        Value::Short(value) => Some(*value as i32),
        Value::Int(value) => Some(*value),
        Value::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn value_f64(value: &Value) -> Option<f64> {
    match value {
        Value::Byte(value) => Some(*value as f64),
        Value::Short(value) => Some(*value as f64),
        Value::Int(value) => Some(*value as f64),
        Value::Long(value) => Some(*value as f64),
        Value::Float(value) => Some(*value as f64),
        Value::Double(value) => Some(*value),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum StructureError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("NBT 解析失败: {0}")]
    Nbt(fastnbt::error::Error),
    #[error("无法识别结构格式: {0}")]
    UnknownFormat(PathBuf),
    #[error("只有 Minecraft 世界目录支持 region 读取范围")]
    RegionOnlyForWorld,
    #[error("缺少字段: {0}")]
    MissingField(String),
    #[error("NBT 字段类型无效, 期望 {0}")]
    InvalidType(&'static str),
    #[error("调色板索引越界: {0}")]
    InvalidPaletteIndex(usize),
    #[error("Sponge schematic 无效: {0}")]
    InvalidSpongeSchematic(String),
    #[error("方块状态解析失败: {0}")]
    Resolve(String),
    #[error("Minecraft 世界读取失败: {0}")]
    MinecraftWorld(String),
    #[error(transparent)]
    World(#[from] redstone_core::WorldError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_mirrors_before_rotating() {
        let transform = StructureTransform {
            origin: BlockPos::new(10, 20, 30),
            mirror: Mirror::FrontBack,
            rotation: Rotation::Clockwise90,
        };
        assert_eq!(
            transform.apply(BlockPos::new(2, 3, 4)),
            BlockPos::new(6, 23, 28)
        );
    }
}
