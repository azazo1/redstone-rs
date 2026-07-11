use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use zip::ZipArchive;

pub(super) trait ReadSeek: Read + Seek {}

impl<T: Read + Seek> ReadSeek for T {}

pub(super) enum WorldSource {
    Directory {
        root: PathBuf,
    },
    Zip {
        path: PathBuf,
        archive: ZipArchive<File>,
        prefix: String,
        files: BTreeMap<String, u64>,
    },
}

impl WorldSource {
    pub fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Ok(Self::Directory {
                root: path.to_path_buf(),
            });
        }
        if path.extension().is_none_or(|extension| extension != "zip") {
            return Err(format!("世界输入不是目录或 ZIP: {}", path.display()));
        }
        let file = File::open(path)
            .map_err(|error| format!("打开世界 ZIP {} 失败: {error}", path.display()))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("读取世界 ZIP {} 失败: {error}", path.display()))?;
        let mut files = BTreeMap::new();
        for index in 0..archive.len() {
            let entry = archive
                .by_index(index)
                .map_err(|error| format!("读取 ZIP entry {index} 失败: {error}"))?;
            if entry.is_dir() {
                continue;
            }
            let Some(enclosed) = entry.enclosed_name() else {
                return Err(format!("ZIP 包含不安全路径: {}", entry.name()));
            };
            files.insert(normalize(&enclosed), entry.size());
        }
        let suffix = "data/minecraft/world_gen_settings.dat";
        let roots = files
            .keys()
            .filter_map(|name| {
                if name == suffix {
                    Some(String::new())
                } else {
                    name.strip_suffix(&format!("/{suffix}"))
                        .map(str::to_owned)
                }
            })
            .collect::<BTreeSet<_>>();
        if roots.len() != 1 {
            return Err(format!(
                "世界 ZIP 必须包含唯一的 {suffix}, 找到 {} 个",
                roots.len()
            ));
        }
        let prefix = roots.into_iter().next().unwrap();
        if prefix.contains('/') {
            return Err(format!("世界 ZIP 最多只能包含一个顶层文件夹: {prefix}"));
        }
        Ok(Self::Zip {
            path: path.to_path_buf(),
            archive,
            prefix,
            files,
        })
    }

    pub fn read(&mut self, relative: &Path) -> Result<Vec<u8>, String> {
        match self {
            Self::Directory { root } => {
                let path = root.join(relative);
                std::fs::read(&path)
                    .map_err(|error| format!("读取 {} 失败: {error}", path.display()))
            }
            Self::Zip {
                path,
                archive,
                prefix,
                ..
            } => {
                let name = archive_name(prefix, relative);
                let mut entry = archive.by_name(&name).map_err(|error| {
                    format!("读取 ZIP entry {}!/{name} 失败: {error}", path.display())
                })?;
                let mut bytes = Vec::new();
                entry.read_to_end(&mut bytes).map_err(|error| {
                    format!("解压 ZIP entry {}!/{name} 失败: {error}", path.display())
                })?;
                Ok(bytes)
            }
        }
    }

    pub fn open_region(&mut self, relative: &Path) -> Result<Box<dyn ReadSeek>, String> {
        match self {
            Self::Directory { root } => {
                let path = root.join(relative);
                File::open(&path)
                    .map(|file| Box::new(file) as Box<dyn ReadSeek>)
                    .map_err(|error| format!("打开 region {} 失败: {error}", path.display()))
            }
            Self::Zip { .. } => self
                .read(relative)
                .map(|bytes| Box::new(Cursor::new(bytes)) as Box<dyn ReadSeek>),
        }
    }

    pub fn list_files(&self, relative_directory: &Path) -> Result<Vec<PathBuf>, String> {
        match self {
            Self::Directory { root } => {
                let directory = root.join(relative_directory);
                if !directory.is_dir() {
                    return Ok(Vec::new());
                }
                let mut files = Vec::new();
                for entry in std::fs::read_dir(&directory).map_err(|error| error.to_string())? {
                    let entry = entry.map_err(|error| error.to_string())?;
                    if entry.path().is_file()
                        && entry.metadata().is_ok_and(|metadata| metadata.len() >= 8192)
                    {
                        files.push(relative_directory.join(entry.file_name()));
                    }
                }
                files.sort();
                Ok(files)
            }
            Self::Zip { prefix, files, .. } => {
                let directory = archive_name(prefix, relative_directory);
                let directory = format!("{}/", directory.trim_end_matches('/'));
                let mut entries = files
                    .iter()
                    .filter_map(|(name, size)| {
                        if *size < 8192 {
                            return None;
                        }
                        let rest = name.strip_prefix(&directory)?;
                        (!rest.contains('/')).then(|| relative_directory.join(rest))
                    })
                    .collect::<Vec<_>>();
                entries.sort();
                Ok(entries)
            }
        }
    }

    pub fn has_directory(&self, relative_directory: &Path) -> bool {
        match self {
            Self::Directory { root } => root.join(relative_directory).is_dir(),
            Self::Zip { prefix, files, .. } => {
                let directory = archive_name(prefix, relative_directory);
                let directory = format!("{}/", directory.trim_end_matches('/'));
                files.keys().any(|name| name.starts_with(&directory))
            }
        }
    }

    pub fn display(&self, relative: &Path) -> String {
        match self {
            Self::Directory { root } => root.join(relative).display().to_string(),
            Self::Zip { path, prefix, .. } => {
                format!("{}!/{}", path.display(), archive_name(prefix, relative))
            }
        }
    }
}

fn archive_name(prefix: &str, relative: &Path) -> String {
    let relative = normalize(relative);
    if prefix.is_empty() {
        relative
    } else if relative.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}/{relative}")
    }
}

fn normalize(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_names_support_root_and_single_prefix() {
        assert_eq!(archive_name("", Path::new("data/a.dat")), "data/a.dat");
        assert_eq!(
            archive_name("world", Path::new("data/a.dat")),
            "world/data/a.dat"
        );
    }
}
