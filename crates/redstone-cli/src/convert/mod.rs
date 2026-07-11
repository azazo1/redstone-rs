use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use redstone_core::BlockStateId;
use redstone_io::{
    StructureLoadOptions, StructureLoader, StructureRegion, StructureState, StructureWriter,
    StructureWriteOptions,
};
use redstone_java_26::{JAVA_DATA_VERSION, Java26Registry};
use tracing::info;

use crate::{RegistryResolver, reject_newer_data_version};

mod scenario;

pub fn run(input: &Path, output: &Path, region: Option<StructureRegion>) -> Result<()> {
    if input.extension().is_some_and(|extension| extension == "toml") {
        if region.is_some() {
            bail!("场景转换的 region 必须写在 source 或 paste 中");
        }
        return scenario::convert(input, output);
    }
    if output.extension().is_some_and(|extension| extension == "toml") {
        bail!("只有场景 TOML 可以转换为 TOML");
    }
    convert_structure(input, output, region, true).map(|_| ())
}

pub(super) fn convert_structure(
    input: &Path,
    output: &Path,
    region: Option<StructureRegion>,
    include_air: bool,
) -> Result<redstone_io::LoadedStructure> {
    let mut resolver = RegistryResolver(Java26Registry::new());
    let structure = StructureLoader::load_with_options(
        input,
        StructureLoadOptions {
            region,
            ..StructureLoadOptions::default()
        },
        &mut resolver,
    )?;
    reject_newer_data_version(structure.data_version)?;
    write_structure(output, &structure, include_air, &resolver.0)?;
    info!(
        input = %input.display(),
        output = %output.display(),
        blocks = structure.world.non_air_blocks(),
        "完成结构转换"
    );
    Ok(structure)
}

pub(super) fn write_structure(
    output: &Path,
    structure: &redstone_io::LoadedStructure,
    include_air: bool,
    registry: &Java26Registry,
) -> Result<()> {
    StructureWriter::write(
        output,
        structure,
        StructureWriteOptions {
            include_air,
            data_version: JAVA_DATA_VERSION,
        },
        |id: BlockStateId| {
            let state = registry
                .state(id)
                .ok_or_else(|| format!("未知方块状态 {id:?}"))?;
            Ok(StructureState {
                name: state.name.to_string(),
                properties: state.properties.as_ref().clone(),
            })
        },
    )
    .with_context(|| format!("写入结构失败: {}", output.display()))
}

pub fn converter_executable() -> Result<PathBuf> {
    std::env::current_exe().map_err(|error| anyhow!(error))
}
