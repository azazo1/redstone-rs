use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use fastnbt::Value;
use redstone_core::{BlockPos, SparseWorld};
use tracing::info;

use super::{
    LoadedStructure, StructureError, StructureLoadOptions, StructureRegion,
    StructureStateResolver, transformed_bounds, update_bounds,
};

mod chunk;
mod region;
mod settings;
mod source;

pub(super) fn load<R: StructureStateResolver>(
    path: &Path,
    options: StructureLoadOptions,
    resolver: &mut R,
) -> Result<LoadedStructure, StructureError> {
    let mut source = source::WorldSource::open(path).map_err(StructureError::MinecraftWorld)?;
    let settings_path = Path::new("data/minecraft/world_gen_settings.dat");
    let settings_display = source.display(settings_path);
    let settings_bytes = source.read(settings_path).map_err(StructureError::MinecraftWorld)?;
    let settings = settings::read(settings_bytes, &settings_display)
        .map_err(StructureError::MinecraftWorld)?;
    settings::require_data_version(&settings, &settings_display)
        .map_err(StructureError::MinecraftWorld)?;
    let explicit_region = options.region;
    if explicit_region.is_none() && !settings::is_strict_void(&settings) {
        return Err(StructureError::MinecraftWorld(
            "世界不是可证明的严格虚空世界, 必须提供有限 region".to_owned(),
        ));
    }
    let dimension = Path::new("dimensions/minecraft/overworld");
    let block_regions = region::list(&source, &dimension.join("region"))
        .map_err(StructureError::MinecraftWorld)?;
    let entity_directory = dimension.join("entities");
    let entity_regions = region::list(&source, &entity_directory)
        .map_err(StructureError::MinecraftWorld)?;
    let separate_entities = source.has_directory(&entity_directory);
    info!(
        world = %path.display(),
        regions = block_regions.len(),
        entity_regions = entity_regions.len(),
        explicit_region = explicit_region.is_some(),
        "开始读取 Minecraft 世界"
    );
    let mut state = LoadState::new(resolver.air_state());
    let mut chunks = 0usize;
    for region_path in &block_regions {
        if !region_intersects(region_path.x, region_path.z, explicit_region) {
            continue;
        }
        let region_display = source.display(&region_path.path);
        chunks += region::read_chunks(
            &mut source,
            region_path,
            |chunk_x, chunk_z| chunk_intersects(chunk_x, chunk_z, explicit_region),
            |chunk_x, chunk_z, bytes| {
                let root = fastnbt::from_bytes::<HashMap<String, Value>>(&bytes)
                    .map_err(|error| error.to_string())?;
                settings::require_data_version(&root, &region_display)?;
                chunk::load_chunk(
                    &root,
                    (chunk_x, chunk_z),
                    chunk::ChunkLoadOptions {
                        region: explicit_region,
                        load: options,
                        separate_entities,
                    },
                    resolver,
                    &mut state,
                )
            },
        )
        .map_err(StructureError::MinecraftWorld)?;
        info!(
            path = %region_display,
            chunks,
            blocks = state.world.non_air_blocks(),
            "读取世界 region"
        );
    }
    if separate_entities {
        for region_path in &entity_regions {
            if !region_intersects(region_path.x, region_path.z, explicit_region) {
                continue;
            }
            let region_display = source.display(&region_path.path);
            region::read_chunks(
                &mut source,
                region_path,
                |chunk_x, chunk_z| chunk_intersects(chunk_x, chunk_z, explicit_region),
                |_chunk_x, _chunk_z, bytes| {
                    let root = fastnbt::from_bytes::<HashMap<String, Value>>(&bytes)
                        .map_err(|error| error.to_string())?;
                    settings::require_data_version(&root, &region_display)?;
                    chunk::load_entities(&root, "Entities", explicit_region, options, &mut state)
                },
            )
            .map_err(StructureError::MinecraftWorld)?;
        }
    }
    let (region_min, region_max) = if let Some(region) = explicit_region {
        transformed_bounds(options.transform, region.min, region.max)
    } else {
        (
            state.content_min.unwrap_or(options.transform.origin),
            state.content_max.unwrap_or(options.transform.origin),
        )
    };
    let min = state.block_min.unwrap_or(region_min);
    let max = state.block_max.unwrap_or(region_max);
    info!(
        world = %path.display(),
        chunks,
        blocks = state.world.non_air_blocks(),
        entities = state.world.entities().count(),
        ?region_min,
        ?region_max,
        "完成 Minecraft 世界读取"
    );
    Ok(LoadedStructure {
        world: state.world,
        min,
        max,
        region_min,
        region_max,
        format: "minecraft_world".to_owned(),
        data_version: Some(settings::DATA_VERSION),
        block_counts: state.block_counts,
        block_entity_nbt: state.block_entity_nbt,
    })
}

struct LoadState {
    world: SparseWorld,
    block_counts: BTreeMap<String, usize>,
    block_entity_nbt: BTreeMap<BlockPos, BTreeMap<String, serde_json::Value>>,
    block_min: Option<BlockPos>,
    block_max: Option<BlockPos>,
    content_min: Option<BlockPos>,
    content_max: Option<BlockPos>,
}

impl LoadState {
    fn new(air: redstone_core::BlockStateId) -> Self {
        Self {
            world: SparseWorld::new(air),
            block_counts: BTreeMap::new(),
            block_entity_nbt: BTreeMap::new(),
            block_min: None,
            block_max: None,
            content_min: None,
            content_max: None,
        }
    }

    fn include_block(&mut self, pos: BlockPos) {
        update_bounds(&mut self.block_min, &mut self.block_max, pos);
        self.include_content(pos);
    }

    fn include_content(&mut self, pos: BlockPos) {
        update_bounds(&mut self.content_min, &mut self.content_max, pos);
    }
}

fn region_intersects(x: i32, z: i32, region: Option<StructureRegion>) -> bool {
    region.is_none_or(|region| {
        let min_x = x.saturating_mul(512);
        let min_z = z.saturating_mul(512);
        let max_x = min_x.saturating_add(511);
        let max_z = min_z.saturating_add(511);
        min_x <= region.max.x
            && max_x >= region.min.x
            && min_z <= region.max.z
            && max_z >= region.min.z
    })
}

fn chunk_intersects(x: i32, z: i32, region: Option<StructureRegion>) -> bool {
    region.is_none_or(|region| {
        let min_x = x.saturating_mul(16);
        let min_z = z.saturating_mul(16);
        let max_x = min_x.saturating_add(15);
        let max_z = min_z.saturating_add(15);
        min_x <= region.max.x
            && max_x >= region.min.x
            && min_z <= region.max.z
            && max_z >= region.min.z
    })
}
