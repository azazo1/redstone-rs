use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use redstone_core::BlockStateId;
use thiserror::Error;
use tracing::info;

use super::{
    LoadedStructure, StructureFormat, StructureLoader, VanillaWriteError, litematic_writer,
    sponge_writer, vanilla_writer,
};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct StructureState {
    pub name: String,
    pub properties: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructureWriteOptions {
    pub include_air: bool,
    pub data_version: i32,
}

impl StructureWriteOptions {
    pub const fn new(data_version: i32) -> Self {
        Self {
            include_air: true,
            data_version,
        }
    }
}

pub struct StructureWriter;

impl StructureWriter {
    pub fn write(
        path: impl AsRef<Path>,
        structure: &LoadedStructure,
        options: StructureWriteOptions,
        mut describe_state: impl FnMut(BlockStateId) -> Result<StructureState, String>,
    ) -> Result<(), StructureWriteError> {
        let path = path.as_ref();
        let format = StructureLoader::detect(path)?;
        if format == StructureFormat::MinecraftWorld {
            return Err(StructureWriteError::WorldOutput(path.to_path_buf()));
        }
        let bytes = match format {
            StructureFormat::VanillaStructure => vanilla_writer::encode_vanilla_structure(
                structure,
                options.include_air,
                options.data_version,
                &mut describe_state,
            )?,
            StructureFormat::Litematic => litematic_writer::encode(
                structure,
                options.data_version,
                &mut describe_state,
            )?,
            StructureFormat::SpongeSchematic => sponge_writer::encode(
                structure,
                options.data_version,
                &mut describe_state,
            )?,
            StructureFormat::MinecraftWorld => unreachable!(),
        };
        atomic_write(path, &bytes)?;
        info!(path = %path.display(), ?format, bytes = bytes.len(), "写出结构文件");
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), StructureWriteError> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .ok_or_else(|| StructureWriteError::InvalidOutput(path.to_path_buf()))?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok::<_, std::io::Error>(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result.map_err(StructureWriteError::Io)
}

#[derive(Debug, Error)]
pub enum StructureWriteError {
    #[error(transparent)]
    Structure(#[from] super::StructureError),
    #[error(transparent)]
    Vanilla(#[from] VanillaWriteError),
    #[error("编码 Litematic 失败: {0}")]
    Litematic(String),
    #[error("编码 Sponge schematic 失败: {0}")]
    Sponge(String),
    #[error("输出路径是世界目录或目录路径: {0}")]
    WorldOutput(PathBuf),
    #[error("输出路径无效: {0}")]
    InvalidOutput(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
