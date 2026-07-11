use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use redstone_io::{Mirror, Rotation, Scenario, StructureLoader};
use redstone_java_26::Java26Registry;
use tracing::info;

use crate::{RegistryResolver, reject_newer_data_version};

use super::write_structure;

pub(super) fn convert(input: &Path, output: &Path) -> Result<()> {
    if output.extension().is_none_or(|extension| extension != "toml") {
        bail!("场景转换输出必须使用 .toml 扩展名");
    }
    let mut scenario = Scenario::load(input)?;
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(output_parent).with_context(|| {
        format!("创建场景转换目录失败: {}", output_parent.display())
    })?;
    let stem = output
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("scenario");
    let assets_name = format!("{stem}-assets");
    let assets_directory = output_parent.join(&assets_name);
    std::fs::create_dir_all(&assets_directory).with_context(|| {
        format!("创建场景结构目录失败: {}", assets_directory.display())
    })?;
    let mut resolver = RegistryResolver(Java26Registry::new());

    let source = StructureLoader::load_with_options(
        &scenario.source.path,
        scenario.source.load_options(),
        &mut resolver,
    )?;
    reject_newer_data_version(source.data_version)?;
    let source_relative = PathBuf::from(&assets_name).join("source.nbt");
    write_structure(
        &output_parent.join(&source_relative),
        &source,
        false,
        &resolver.0,
    )?;
    scenario.source.path = source_relative;
    scenario.source.origin = source.region_min;
    scenario.source.rotation = Rotation::None;
    scenario.source.mirror = Mirror::None;
    scenario.source.region = None;

    for (index, paste) in scenario.source.pastes.iter_mut().enumerate() {
        let structure = StructureLoader::load_with_options(
            &paste.path,
            paste.load_options(),
            &mut resolver,
        )?;
        reject_newer_data_version(structure.data_version)?;
        let relative = PathBuf::from(&assets_name).join(format!("paste-{index}.nbt"));
        write_structure(
            &output_parent.join(&relative),
            &structure,
            !paste.ignore_air,
            &resolver.0,
        )?;
        paste.path = relative;
        paste.origin = structure.region_min;
        paste.rotation = Rotation::None;
        paste.mirror = Mirror::None;
        paste.region = None;
    }

    let document = toml::to_string_pretty(&scenario).context("序列化转换场景失败")?;
    std::fs::write(output, document)
        .with_context(|| format!("写入转换场景失败: {}", output.display()))?;
    info!(input = %input.display(), output = %output.display(), "完成场景转换");
    Ok(())
}
