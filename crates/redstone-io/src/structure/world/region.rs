use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use flate2::read::{GzDecoder, ZlibDecoder};

const HEADER_BYTES: usize = 8192;
const SECTOR_BYTES: u64 = 4096;

#[derive(Clone, Debug)]
pub(super) struct RegionPath {
    pub path: PathBuf,
    pub x: i32,
    pub z: i32,
}

pub(super) fn list(directory: &Path) -> Result<Vec<RegionPath>, String> {
    if !directory.is_dir() {
        return Ok(Vec::new());
    }
    let mut regions = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let Some((x, z)) = parse_name(&path) else {
            continue;
        };
        regions.push(RegionPath { path, x, z });
    }
    regions.sort_by_key(|region| (region.x, region.z));
    Ok(regions)
}

pub(super) fn read_chunks(
    region: &RegionPath,
    mut accepts: impl FnMut(i32, i32) -> bool,
    mut consume: impl FnMut(i32, i32, Vec<u8>) -> Result<(), String>,
) -> Result<usize, String> {
    let mut file = File::open(&region.path).map_err(|error| error.to_string())?;
    let mut header = [0u8; HEADER_BYTES];
    file.read_exact(&mut header).map_err(|error| {
        format!("region header 截断 {}: {error}", region.path.display())
    })?;
    let mut count = 0usize;
    for index in 0..1024usize {
        let base = index * 4;
        let sector = u32::from_be_bytes([0, header[base], header[base + 1], header[base + 2]]);
        let sectors = header[base + 3] as u64;
        if sector == 0 || sectors == 0 {
            continue;
        }
        let local_x = (index % 32) as i32;
        let local_z = (index / 32) as i32;
        let chunk_x = region
            .x
            .checked_mul(32)
            .and_then(|value| value.checked_add(local_x))
            .ok_or_else(|| "chunk x 坐标溢出".to_owned())?;
        let chunk_z = region
            .z
            .checked_mul(32)
            .and_then(|value| value.checked_add(local_z))
            .ok_or_else(|| "chunk z 坐标溢出".to_owned())?;
        if !accepts(chunk_x, chunk_z) {
            continue;
        }
        let offset = u64::from(sector) * SECTOR_BYTES;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut chunk_header = [0u8; 5];
        file.read_exact(&mut chunk_header).map_err(|error| {
            format!("chunk header 截断 ({chunk_x},{chunk_z}): {error}")
        })?;
        let length = u32::from_be_bytes(chunk_header[..4].try_into().unwrap()) as usize;
        if length == 0 {
            return Err(format!("chunk ({chunk_x},{chunk_z}) 长度为 0"));
        }
        let version = chunk_header[4];
        let external = version & 0x80 != 0;
        let compression = version & 0x7f;
        let compressed = if external {
            if length != 1 {
                return Err(format!(
                    "外部 chunk ({chunk_x},{chunk_z}) 内部长度必须为 1"
                ));
            }
            let path = region
                .path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(format!("c.{chunk_x}.{chunk_z}.mcc"));
            std::fs::read(&path)
                .map_err(|error| format!("读取外部 chunk {} 失败: {error}", path.display()))?
        } else {
            let compressed_length = length - 1;
            let maximum = sectors
                .checked_mul(SECTOR_BYTES)
                .and_then(|value| value.checked_sub(5))
                .ok_or_else(|| "chunk sector 长度溢出".to_owned())?;
            if compressed_length as u64 > maximum {
                return Err(format!(
                    "chunk ({chunk_x},{chunk_z}) 声明长度超过分配 sector"
                ));
            }
            let mut bytes = vec![0u8; compressed_length];
            file.read_exact(&mut bytes).map_err(|error| {
                format!("chunk 数据截断 ({chunk_x},{chunk_z}): {error}")
            })?;
            bytes
        };
        let decoded = decompress(compression, compressed)
            .map_err(|error| format!("解压 chunk ({chunk_x},{chunk_z}) 失败: {error}"))?;
        consume(chunk_x, chunk_z, decoded)?;
        count += 1;
    }
    Ok(count)
}

fn decompress(compression: u8, bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    match compression {
        1 => GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut output)
            .map_err(|error| error.to_string())?,
        2 => ZlibDecoder::new(bytes.as_slice())
            .read_to_end(&mut output)
            .map_err(|error| error.to_string())?,
        3 => return Ok(bytes),
        4 => lz4_java_wrc::Lz4BlockInput::new(Cursor::new(bytes))
            .read_to_end(&mut output)
            .map_err(|error| error.to_string())?,
        value => return Err(format!("不支持 region 压缩类型 {value}")),
    };
    Ok(output)
}

fn parse_name(path: &Path) -> Option<(i32, i32)> {
    let name = path.file_name()?.to_str()?;
    let mut parts = name.split('.');
    if parts.next()? != "r" {
        return None;
    }
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    if parts.next()? != "mca" || parts.next().is_some() {
        return None;
    }
    Some((x, z))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder, write::ZlibEncoder};
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn parses_negative_region_names() {
        assert_eq!(parse_name(Path::new("r.-2.7.mca")), Some((-2, 7)));
        assert_eq!(parse_name(Path::new("c.0.0.mcc")), None);
    }

    #[test]
    fn decompresses_all_supported_region_compressions() {
        let source = b"minecraft region compression";
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        gzip.write_all(source).unwrap();
        let mut zlib = ZlibEncoder::new(Vec::new(), Compression::default());
        zlib.write_all(source).unwrap();
        let mut lz4 = Vec::new();
        {
            let mut encoder = lz4_java_wrc::Lz4BlockOutput::new(&mut lz4);
            encoder.write_all(source).unwrap();
            encoder.flush().unwrap();
        }
        assert_eq!(decompress(1, gzip.finish().unwrap()).unwrap(), source);
        assert_eq!(decompress(2, zlib.finish().unwrap()).unwrap(), source);
        assert_eq!(decompress(3, source.to_vec()).unwrap(), source);
        assert_eq!(decompress(4, lz4).unwrap(), source);
    }

    #[test]
    fn reads_external_chunk_streams() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "redstone-external-region-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("r.0.0.mca");
        let mut bytes = vec![0u8; HEADER_BYTES + 5];
        bytes[..4].copy_from_slice(&[0, 0, 2, 1]);
        bytes[HEADER_BYTES..HEADER_BYTES + 4].copy_from_slice(&1u32.to_be_bytes());
        bytes[HEADER_BYTES + 4] = 0x83;
        std::fs::write(&path, bytes).unwrap();
        std::fs::write(directory.join("c.0.0.mcc"), b"external").unwrap();
        let region = RegionPath { path, x: 0, z: 0 };
        let mut received = Vec::new();
        let count = read_chunks(
            &region,
            |_, _| true,
            |x, z, bytes| {
                assert_eq!((x, z), (0, 0));
                received = bytes;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(count, 1);
        assert_eq!(received, b"external");
    }
}
