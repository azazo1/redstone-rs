use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use redstone_core::BlockStateId;
use thiserror::Error;
use tracing::{Span, info_span};
use tracing_indicatif::{span_ext::IndicatifSpanExt, style::ProgressStyle};

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

const WRITE_PROGRESS_INTERVAL: u64 = 16_384;

pub(super) struct WriteProgress {
    span: Span,
    position: u64,
    encoding_items: u64,
}

impl WriteProgress {
    fn new(
        path: &Path,
        format: StructureFormat,
        encoding_items: u64,
    ) -> Result<Self, StructureWriteError> {
        let total = encoding_items
            .checked_add(1)
            .ok_or(StructureWriteError::VolumeOverflow)?;
        let file_name = output_file_name(path);
        let span = info_span!("structure_write", path = %path.display());
        span.pb_set_style(&write_progress_style());
        span.pb_set_length(total);
        span.pb_set_message(&format!("编码 {} {file_name}", format_name(format)));
        span.pb_start();
        Ok(Self {
            span,
            position: 0,
            encoding_items,
        })
    }

    pub(super) fn advance_encoding(&mut self) {
        self.position += 1;
        if self.position.is_multiple_of(WRITE_PROGRESS_INTERVAL)
            || self.position == self.encoding_items
        {
            self.span.pb_set_position(self.position);
        }
    }

    fn write_file(&mut self, path: &Path, bytes: &[u8]) -> Result<(), StructureWriteError> {
        let file_name = output_file_name(path);
        self.span.pb_set_message(&format!("写入 {file_name}"));
        self.span.pb_set_finish_message(&format!(
            "完成写入 {file_name}, {} bytes",
            bytes.len(),
        ));
        atomic_write(path, bytes)?;
        self.position += 1;
        self.span.pb_set_position(self.position);
        Ok(())
    }
}

fn write_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{span_child_prefix}{spinner:.green} {msg} [{bar:28.green}] {pos}/{len} {per_sec:2} ETA:{eta}",
    )
    .expect("结构写入进度条模板必须有效")
    .progress_chars("=> ")
}

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
        let volume = region_volume(structure)?;
        let encoding_items = if format == StructureFormat::VanillaStructure && !options.include_air {
            u64::try_from(
                structure
                    .world
                    .iter_blocks()
                    .filter(|(pos, _)| in_region(*pos, structure.region_min, structure.region_max))
                    .count(),
            )
            .map_err(|_| StructureWriteError::VolumeOverflow)?
        } else {
            volume
        };
        let mut progress = WriteProgress::new(path, format, encoding_items)?;
        let bytes = match format {
            StructureFormat::VanillaStructure => vanilla_writer::encode_vanilla_structure(
                structure,
                options.include_air,
                options.data_version,
                &mut progress,
                &mut describe_state,
            )?,
            StructureFormat::Litematic => litematic_writer::encode(
                structure,
                options.data_version,
                &mut progress,
                &mut describe_state,
            )?,
            StructureFormat::SpongeSchematic => sponge_writer::encode(
                structure,
                options.data_version,
                &mut progress,
                &mut describe_state,
            )?,
            StructureFormat::MinecraftWorld => unreachable!(),
        };
        progress.write_file(path, &bytes)
    }
}

fn region_volume(structure: &LoadedStructure) -> Result<u64, StructureWriteError> {
    let axis = |min: i32, max: i32| {
        let length = i64::from(max) - i64::from(min) + 1;
        u64::try_from(length)
            .ok()
            .filter(|length| *length > 0)
            .ok_or(StructureWriteError::InvalidRegionBounds)
    };
    let x = axis(structure.region_min.x, structure.region_max.x)?;
    let y = axis(structure.region_min.y, structure.region_max.y)?;
    let z = axis(structure.region_min.z, structure.region_max.z)?;
    x.checked_mul(y)
        .and_then(|volume| volume.checked_mul(z))
        .ok_or(StructureWriteError::VolumeOverflow)
}

fn in_region(
    pos: redstone_core::BlockPos,
    min: redstone_core::BlockPos,
    max: redstone_core::BlockPos,
) -> bool {
    (min.x..=max.x).contains(&pos.x)
        && (min.y..=max.y).contains(&pos.y)
        && (min.z..=max.z).contains(&pos.z)
}

fn output_file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn format_name(format: StructureFormat) -> &'static str {
    match format {
        StructureFormat::Litematic => "Litematic",
        StructureFormat::SpongeSchematic => "Sponge schematic",
        StructureFormat::VanillaStructure => "vanilla structure",
        StructureFormat::MinecraftWorld => "Minecraft world",
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
    #[error("结构 region 边界无效")]
    InvalidRegionBounds,
    #[error("结构 region 体积溢出")]
    VolumeOverflow,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
