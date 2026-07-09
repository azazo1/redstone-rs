use std::collections::BTreeMap;
use std::io::Read;

use fastnbt::{from_bytes, ByteArray, LongArray};
use flate2::read::{GzDecoder, ZlibDecoder};
use serde::Deserialize;

use crate::core::{BlockKind, BlockState, Position};

use super::{structure::state_from_parts, StructureBlock, StructureError, StructureInput};

#[derive(Debug, Deserialize)]
struct RawPaletteEntry {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Properties", default)]
    properties: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct RawStructure {
    palette: Vec<RawPaletteEntry>,
    blocks: Vec<RawStructureBlock>,
}

#[derive(Debug, Deserialize)]
struct RawStructureBlock {
    pos: Vec<i32>,
    state: usize,
}

#[derive(Debug, Deserialize)]
struct RawSpongeSchematic {
    #[serde(rename = "Width")]
    width: i16,
    #[serde(rename = "Height")]
    height: i16,
    #[serde(rename = "Length")]
    length: i16,
    #[serde(rename = "Palette")]
    palette: BTreeMap<String, i32>,
    #[serde(rename = "BlockData")]
    block_data: ByteArray,
}

#[derive(Debug, Deserialize)]
struct RawLitematic {
    #[serde(rename = "Regions")]
    regions: BTreeMap<String, RawLitematicRegion>,
}

#[derive(Debug, Deserialize)]
struct RawLitematicRegion {
    #[serde(rename = "Position")]
    position: RawVec3,
    #[serde(rename = "Size")]
    size: RawVec3,
    #[serde(rename = "BlockStatePalette")]
    palette: Vec<RawPaletteEntry>,
    #[serde(rename = "BlockStates")]
    block_states: LongArray,
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct RawVec3 {
    x: i32,
    y: i32,
    z: i32,
}

#[derive(Debug, Deserialize)]
struct RawChunk {
    #[serde(rename = "xPos")]
    x: i32,
    #[serde(rename = "zPos")]
    z: i32,
    sections: Vec<RawChunkSection>,
}

#[derive(Debug, Deserialize)]
struct RawChunkSection {
    #[serde(rename = "Y")]
    y: i8,
    #[serde(rename = "block_states")]
    block_states: Option<RawPalettedBlockStates>,
}

#[derive(Debug, Deserialize)]
struct RawPalettedBlockStates {
    palette: Vec<RawPaletteEntry>,
    #[serde(default)]
    data: Option<LongArray>,
}

pub(super) fn decode(bytes: &[u8]) -> Result<StructureInput, StructureError> {
    let bytes = decompress_if_needed(bytes)?;
    if let Ok(raw) = from_bytes::<RawStructure>(&bytes) {
        return decode_structure(raw);
    }
    if let Ok(raw) = from_bytes::<RawSpongeSchematic>(&bytes) {
        return decode_sponge(raw);
    }
    if let Ok(raw) = from_bytes::<RawLitematic>(&bytes) {
        return decode_litematic(raw);
    }
    Err(StructureError::Nbt("unsupported NBT structure format".to_owned()))
}

pub(super) fn decode_anvil_region(bytes: &[u8]) -> Result<StructureInput, StructureError> {
    if bytes.len() < 8_192 {
        return Err(StructureError::Nbt("anvil region header is truncated".to_owned()));
    }
    let mut blocks = Vec::new();
    for index in 0..1_024usize {
        let offset = index * 4;
        let location = u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("location size is fixed"));
        let sector_offset = (location >> 8) as usize;
        let sector_count = (location & 0xff) as usize;
        if sector_offset == 0 || sector_count == 0 {
            continue;
        }
        let start = sector_offset.checked_mul(4_096).ok_or(StructureError::InvalidDimensions)?;
        let available = sector_count.checked_mul(4_096).ok_or(StructureError::InvalidDimensions)?;
        let header_end = start.checked_add(5).ok_or(StructureError::InvalidDimensions)?;
        if header_end > bytes.len() || available < 5 {
            return Err(StructureError::Nbt("anvil chunk location is invalid".to_owned()));
        }
        let length = u32::from_be_bytes(bytes[start..start + 4].try_into().expect("chunk length is fixed")) as usize;
        if length == 0 || length + 4 > available || start + 4 + length > bytes.len() {
            return Err(StructureError::Nbt("anvil chunk length is invalid".to_owned()));
        }
        let compression = bytes[start + 4];
        let payload = &bytes[start + 5..start + 4 + length];
        let decoded = decode_chunk_payload(compression, payload)?;
        let chunk: RawChunk = from_bytes(&decoded).map_err(|error| StructureError::Nbt(error.to_string()))?;
        decode_chunk(chunk, &mut blocks)?;
    }
    Ok(StructureInput { blocks })
}

fn decompress_if_needed(bytes: &[u8]) -> Result<Vec<u8>, StructureError> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut decoded)
            .map_err(StructureError::Read)?;
        Ok(decoded)
    } else {
        Ok(bytes.to_vec())
    }
}

fn decode_chunk_payload(compression: u8, payload: &[u8]) -> Result<Vec<u8>, StructureError> {
    match compression {
        1 => {
            let mut decoder = GzDecoder::new(payload);
            read_decoder(&mut decoder)
        }
        2 => {
            let mut decoder = ZlibDecoder::new(payload);
            read_decoder(&mut decoder)
        }
        3 => Ok(payload.to_vec()),
        _ => Err(StructureError::Nbt(format!("unsupported anvil compression type {compression}"))),
    }
}

fn read_decoder(reader: &mut impl Read) -> Result<Vec<u8>, StructureError> {
    let mut output = Vec::new();
    reader.read_to_end(&mut output).map_err(StructureError::Read)?;
    Ok(output)
}

fn decode_structure(raw: RawStructure) -> Result<StructureInput, StructureError> {
    let palette = raw
        .palette
        .iter()
        .map(|entry| state_from_parts(&entry.name, &entry.properties))
        .collect::<Result<Vec<_>, _>>()?;
    let mut blocks = Vec::with_capacity(raw.blocks.len());
    for block in raw.blocks {
        let [x, y, z] = block.pos.as_slice() else {
            return Err(StructureError::InvalidPosition);
        };
        let state = palette.get(block.state).copied().ok_or(StructureError::InvalidPosition)?;
        push_non_air(&mut blocks, Position::new(*x, *y, *z), state);
    }
    Ok(StructureInput { blocks })
}

fn decode_sponge(raw: RawSpongeSchematic) -> Result<StructureInput, StructureError> {
    let dimensions = dimensions(raw.width, raw.height, raw.length)?;
    let mut palette = vec![BlockState::default(); raw.palette.len()];
    for (state, index) in raw.palette {
        let index = usize::try_from(index).map_err(|_| StructureError::InvalidPosition)?;
        if index >= palette.len() {
            return Err(StructureError::InvalidPosition);
        }
        palette[index] = parse_state_string(&state)?;
    }
    let data = raw.block_data.into_inner().into_iter().map(|value| value as u8).collect::<Vec<_>>();
    let mut cursor = 0usize;
    let mut blocks = Vec::with_capacity(dimensions.volume);
    for index in 0..dimensions.volume {
        let palette_index = read_varint(&data, &mut cursor)?;
        let state = palette.get(palette_index).copied().ok_or(StructureError::InvalidPosition)?;
        let x = (index % dimensions.width) as i32;
        let z = (index / dimensions.width % dimensions.length) as i32;
        let y = (index / (dimensions.width * dimensions.length)) as i32;
        push_non_air(&mut blocks, Position::new(x, y, z), state);
    }
    Ok(StructureInput { blocks })
}

fn decode_litematic(raw: RawLitematic) -> Result<StructureInput, StructureError> {
    let mut blocks = Vec::new();
    for region in raw.regions.into_values() {
        let dimensions = dimensions_i32(region.size.x, region.size.y, region.size.z)?;
        let palette = region
            .palette
            .iter()
            .map(|entry| state_from_parts(&entry.name, &entry.properties))
            .collect::<Result<Vec<_>, _>>()?;
        let bits = palette_bits(palette.len());
        let states = region.block_states.into_inner();
        for index in 0..dimensions.volume {
            let palette_index = unpack_state(&states, index, bits)?;
            let state = palette.get(palette_index).copied().ok_or(StructureError::InvalidPosition)?;
            let x = coordinate(region.position.x, region.size.x, index % dimensions.width);
            let z = coordinate(region.position.z, region.size.z, index / dimensions.width % dimensions.length);
            let y = coordinate(region.position.y, region.size.y, index / (dimensions.width * dimensions.length));
            push_non_air(&mut blocks, Position::new(x, y, z), state);
        }
    }
    Ok(StructureInput { blocks })
}

fn decode_chunk(chunk: RawChunk, blocks: &mut Vec<StructureBlock>) -> Result<(), StructureError> {
    for section in chunk.sections {
        let Some(block_states) = section.block_states else {
            continue;
        };
        let palette = block_states
            .palette
            .iter()
            .map(|entry| state_from_parts(&entry.name, &entry.properties))
            .collect::<Result<Vec<_>, _>>()?;
        let bits = palette_bits(block_states.palette.len()).max(4);
        let data = block_states.data.map(LongArray::into_inner).unwrap_or_default();
        for index in 0..4_096usize {
            let palette_index = if palette.len() == 1 {
                0
            } else {
                unpack_state(&data, index, bits)?
            };
            let state = palette.get(palette_index).copied().ok_or(StructureError::InvalidPosition)?;
            let x = chunk.x * 16 + (index & 15) as i32;
            let z = chunk.z * 16 + ((index >> 4) & 15) as i32;
            let y = section.y as i32 * 16 + ((index >> 8) & 15) as i32;
            push_non_air(blocks, Position::new(x, y, z), state);
        }
    }
    Ok(())
}

fn push_non_air(blocks: &mut Vec<StructureBlock>, position: Position, state: BlockState) {
    if state.kind != BlockKind::Air {
        blocks.push(StructureBlock { position, state });
    }
}

fn parse_state_string(value: &str) -> Result<BlockState, StructureError> {
    let Some((name, property_text)) = value.split_once('[') else {
        return state_from_parts(value, &BTreeMap::new());
    };
    let property_text = property_text.strip_suffix(']').ok_or_else(|| StructureError::UnsupportedBlock(value.to_owned()))?;
    let mut properties = BTreeMap::new();
    for entry in property_text.split(',') {
        let Some((key, value)) = entry.split_once('=') else {
            return Err(StructureError::UnsupportedBlock(value.to_owned()));
        };
        properties.insert(key.to_owned(), value.to_owned());
    }
    state_from_parts(name, &properties)
}

fn read_varint(data: &[u8], cursor: &mut usize) -> Result<usize, StructureError> {
    let mut value = 0usize;
    for shift in 0..5 {
        let byte = *data.get(*cursor).ok_or(StructureError::TruncatedBlockData)?;
        *cursor += 1;
        value |= ((byte & 0x7f) as usize) << (shift * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(StructureError::TruncatedBlockData)
}

fn unpack_state(states: &[i64], index: usize, bits: usize) -> Result<usize, StructureError> {
    let bit_index = index.checked_mul(bits).ok_or(StructureError::InvalidDimensions)?;
    let long_index = bit_index / 64;
    let offset = bit_index % 64;
    let first = *states.get(long_index).ok_or(StructureError::TruncatedBlockData)? as u64;
    let mask = (1u64 << bits) - 1;
    let value = if offset + bits <= 64 {
        first >> offset
    } else {
        let second = *states.get(long_index + 1).ok_or(StructureError::TruncatedBlockData)? as u64;
        (first >> offset) | (second << (64 - offset))
    };
    Ok((value & mask) as usize)
}

fn palette_bits(length: usize) -> usize {
    let used_bits = usize::BITS as usize - length.saturating_sub(1).leading_zeros() as usize;
    used_bits.max(2)
}

fn coordinate(origin: i32, size: i32, index: usize) -> i32 {
    if size < 0 {
        origin - index as i32
    } else {
        origin + index as i32
    }
}

struct Dimensions {
    width: usize,
    length: usize,
    volume: usize,
}

fn dimensions(width: i16, height: i16, length: i16) -> Result<Dimensions, StructureError> {
    dimensions_i32(width as i32, height as i32, length as i32)
}

fn dimensions_i32(width: i32, height: i32, length: i32) -> Result<Dimensions, StructureError> {
    let width = usize::try_from(width.unsigned_abs()).map_err(|_| StructureError::InvalidDimensions)?;
    let height = usize::try_from(height.unsigned_abs()).map_err(|_| StructureError::InvalidDimensions)?;
    let length = usize::try_from(length.unsigned_abs()).map_err(|_| StructureError::InvalidDimensions)?;
    let volume = width
        .checked_mul(height)
        .and_then(|value| value.checked_mul(length))
        .filter(|value| *value > 0)
        .ok_or(StructureError::InvalidDimensions)?;
    Ok(Dimensions {
        width,
        length,
        volume,
    })
}
