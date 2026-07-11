use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use fastnbt::Value;
use redstone_core::{BlockPos, SparseWorld};
use tracing::{info_span, warn};
use tracing_indicatif::{span_ext::IndicatifSpanExt, style::ProgressStyle};

use super::{
    LoadedStructure, StructureError, StructureLoadOptions, StructureRegion,
    StructureStateResolver, transformed_bounds, update_bounds,
};

mod chunk;
mod region;
mod settings;
mod source;

fn region_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{span_child_prefix}{spinner:.green} {msg} [{bar:28.green}] {pos}/{len} {per_sec:2} ETA:{eta}",
    )
    .expect("世界 region 进度条模板必须有效")
    .progress_chars("=> ")
}

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
    let selected_block_regions = block_regions
        .iter()
        .filter(|region_path| region_intersects(region_path.x, region_path.z, explicit_region))
        .count();
    let selected_entity_regions = if separate_entities {
        entity_regions
            .iter()
            .filter(|region_path| region_intersects(region_path.x, region_path.z, explicit_region))
            .count()
    } else {
        0
    };
    let total_regions = selected_block_regions + selected_entity_regions;
    let progress = info_span!("minecraft_world_regions", world = %path.display());
    progress.pb_set_style(&region_progress_style());
    progress.pb_set_length(total_regions as u64);
    progress.pb_set_message("读取 Minecraft block regions");
    progress.pb_start();
    let mut processed_regions = 0usize;
    let mut state = LoadState::new(resolver.air_state());
    let mut chunks = 0usize;
    for region_path in &block_regions {
        if !region_intersects(region_path.x, region_path.z, explicit_region) {
            continue;
        }
        let region_display = source.display(&region_path.path);
        progress.pb_set_message(&format!(
            "读取 block region {}",
            region_file_name(region_path),
        ));
        if options.skip_old_regions {
            let old_version = old_region_data_version(
                &mut source,
                region_path,
                explicit_region,
            )
            .map_err(StructureError::MinecraftWorld)?;
            if let Some(data_version) = old_version {
                warn!(
                    path = %region_display,
                    region_x = region_path.x,
                    region_z = region_path.z,
                    data_version,
                    expected_data_version = settings::DATA_VERSION,
                    "跳过旧版 Minecraft block region"
                );
                processed_regions += 1;
                progress.pb_set_position(processed_regions as u64);
                continue;
            }
        }
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
        processed_regions += 1;
        progress.pb_set_position(processed_regions as u64);
    }
    if separate_entities {
        progress.pb_set_message("读取 Minecraft entities regions");
        for region_path in &entity_regions {
            if !region_intersects(region_path.x, region_path.z, explicit_region) {
                continue;
            }
            let region_display = source.display(&region_path.path);
            progress.pb_set_message(&format!(
                "读取 entities region {}",
                region_file_name(region_path),
            ));
            if options.skip_old_regions {
                let old_version = old_region_data_version(
                    &mut source,
                    region_path,
                    explicit_region,
                )
                .map_err(StructureError::MinecraftWorld)?;
                if let Some(data_version) = old_version {
                    warn!(
                        path = %region_display,
                        region_x = region_path.x,
                        region_z = region_path.z,
                        data_version,
                        expected_data_version = settings::DATA_VERSION,
                        "跳过旧版 Minecraft entities region"
                    );
                    processed_regions += 1;
                    progress.pb_set_position(processed_regions as u64);
                    continue;
                }
            }
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
            processed_regions += 1;
            progress.pb_set_position(processed_regions as u64);
        }
    }
    progress.pb_set_finish_message(&format!(
        "完成 Minecraft 世界读取, {chunks} chunks, {} blocks, {} entities",
        state.world.non_air_blocks(),
        state.world.entities().count(),
    ));
    progress.pb_set_position(total_regions as u64);
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

fn old_region_data_version(
    source: &mut source::WorldSource,
    region_path: &region::RegionPath,
    explicit_region: Option<StructureRegion>,
) -> Result<Option<i32>, String> {
    let region_display = source.display(&region_path.path);
    let mut old_version = None;
    let result = region::read_chunks(
        source,
        region_path,
        |chunk_x, chunk_z| chunk_intersects(chunk_x, chunk_z, explicit_region),
        |_chunk_x, _chunk_z, bytes| {
            let root = fastnbt::from_bytes::<HashMap<String, Value>>(&bytes)
                .map_err(|error| error.to_string())?;
            if let Some(data_version) = settings::data_version(&root)
                && data_version < settings::DATA_VERSION
            {
                old_version = Some(data_version);
                return Err("停止预检旧版 region".to_owned());
            }
            settings::require_data_version(&root, &region_display)
        },
    );
    if old_version.is_some() {
        Ok(old_version)
    } else {
        result.map(|_| None)
    }
}

fn region_file_name(region_path: &region::RegionPath) -> String {
    region_path
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| region_path.path.display().to_string())
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
