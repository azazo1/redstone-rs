use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use redstone_io::{
    Mirror, Rotation, Scenario, StructureLoader, VanillaState, encode_vanilla_structure,
};
use redstone_java_26::{JAVA_DATA_VERSION, Java26Registry};
use tracing::info;

use crate::RegistryResolver;

pub fn run(input: &Path, output: &Path) -> Result<()> {
    let mut scenario = Scenario::load(input)?;
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(output_parent).with_context(|| {
        format!("创建 oracle 转换目录失败: {}", output_parent.display())
    })?;
    let output_parent = output_parent.canonicalize().with_context(|| {
        format!("解析 oracle 转换目录失败: {}", output_parent.display())
    })?;
    let mut resolver = RegistryResolver(Java26Registry::new());

    let source = StructureLoader::load_transformed(
        &scenario.source.path,
        scenario.source.transform(),
        &mut resolver,
    )?;
    let source_path = output_parent.join("source.nbt");
    write_structure(&source_path, &source, false, &resolver.0)?;
    scenario.source.path = source_path;
    scenario.source.origin = source.region_min;
    scenario.source.rotation = Rotation::None;
    scenario.source.mirror = Mirror::None;

    for (index, paste) in scenario.source.pastes.iter_mut().enumerate() {
        let structure = StructureLoader::load_transformed(
            &paste.path,
            paste.transform(),
            &mut resolver,
        )?;
        let path = output_parent.join(format!("paste-{index}.nbt"));
        write_structure(&path, &structure, !paste.ignore_air, &resolver.0)?;
        paste.path = path;
        paste.origin = structure.region_min;
        paste.rotation = Rotation::None;
        paste.mirror = Mirror::None;
    }

    let document = toml::to_string_pretty(&scenario).context("序列化 oracle 场景失败")?;
    std::fs::write(output, document)
        .with_context(|| format!("写入 oracle 场景失败: {}", output.display()))?;
    info!(input = %input.display(), output = %output.display(), "完成 oracle 场景转换");
    Ok(())
}

fn write_structure(
    path: &Path,
    structure: &redstone_io::LoadedStructure,
    include_air: bool,
    registry: &Java26Registry,
) -> Result<()> {
    let encoded = encode_vanilla_structure(
        structure,
        include_air,
        JAVA_DATA_VERSION,
        |id| {
            let state = registry
                .state(id)
                .ok_or_else(|| format!("未知方块状态 {id:?}"))?;
            Ok(VanillaState {
                name: state.name.to_string(),
                properties: state.properties.as_ref().clone(),
            })
        },
    )?;
    std::fs::write(path, encoded)
        .with_context(|| format!("写入转换结构失败: {}", path.display()))?;
    info!(path = %path.display(), include_air, "写入 oracle vanilla structure");
    Ok(())
}

pub fn converter_executable() -> Result<PathBuf> {
    std::env::current_exe().map_err(|error| anyhow!(error))
}
