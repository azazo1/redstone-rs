use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use redstone_core::BlockStateId;
use redstone_io::{
    StructureFormat, StructureLoadOptions, StructureLoader, StructureRegion, StructureState,
    StructureWriter, StructureWriteOptions,
};
use redstone_java_26::{JAVA_DATA_VERSION, Java26Registry};
use tracing::info;

use crate::{RegistryResolver, reject_newer_data_version};

mod scenario;

pub fn run(
    input: &Path,
    output: &Path,
    region: Option<StructureRegion>,
    skip_old_regions: bool,
) -> Result<()> {
    let scenario_input = input.extension().is_some_and(|extension| extension == "toml");
    validate_output(output, scenario_input)?;
    if scenario_input {
        if region.is_some() {
            bail!("场景转换的 region 必须写在 source 或 paste 中");
        }
        return scenario::convert(input, output, skip_old_regions);
    }
    convert_structure(input, output, region, skip_old_regions, true).map(|_| ())
}

fn validate_output(output: &Path, scenario_input: bool) -> Result<()> {
    let toml_output = output.extension().is_some_and(|extension| extension == "toml");
    if scenario_input {
        if !toml_output {
            bail!("场景转换输出必须使用 .toml 扩展名");
        }
        return Ok(());
    }
    if toml_output {
        bail!("只有场景 TOML 可以转换为 TOML");
    }
    if StructureLoader::detect(output)? == StructureFormat::MinecraftWorld {
        bail!("结构转换输出必须是 .litematic, .schem, .nbt 或 .structure 文件");
    }
    Ok(())
}

pub(super) fn convert_structure(
    input: &Path,
    output: &Path,
    region: Option<StructureRegion>,
    skip_old_regions: bool,
    include_air: bool,
) -> Result<redstone_io::LoadedStructure> {
    let mut resolver = RegistryResolver(Java26Registry::new());
    let structure = StructureLoader::load_with_options(
        input,
        StructureLoadOptions {
            region,
            skip_old_regions,
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
