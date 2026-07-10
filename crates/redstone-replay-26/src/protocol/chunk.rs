use std::collections::{BTreeMap, HashMap};

use redstone_core::{BlockEntityData, BlockPos, BlockStateId};

use super::block_entity::{BlockEntityEncodeError, write_chunk_block_entity};
use super::buf::PacketBuf;

pub(crate) const MIN_Y: i32 = -64;
pub(crate) const MAX_Y: i32 = 319;
pub(crate) const MIN_SECTION_Y: i32 = -4;
pub(crate) const SECTION_COUNT: usize = 24;
pub(crate) const PLAINS_BIOME_ID: u32 = 40;
pub(crate) const MAX_BLOCK_STATE_ID: u32 = 29_872;
const BLOCK_GLOBAL_BITS: u8 = 15;
const HEIGHTMAP_BITS: u8 = 9;
const LIGHT_SECTION_COUNT: usize = SECTION_COUNT + 2;
const LIGHT_BYTES: usize = 2_048;

#[derive(Clone, Debug, Default)]
pub(crate) struct ChunkSnapshot {
    sections: BTreeMap<i32, Vec<BlockStateId>>,
    block_entities: BTreeMap<BlockPos, BlockEntityData>,
}

impl ChunkSnapshot {
    pub(crate) fn set_block(&mut self, pos: BlockPos, state: BlockStateId) {
        let section_y = pos.y.div_euclid(16);
        let section = self
            .sections
            .entry(section_y)
            .or_insert_with(|| vec![BlockStateId(0); 4_096]);
        let x = pos.x.rem_euclid(16) as usize;
        let y = pos.y.rem_euclid(16) as usize;
        let z = pos.z.rem_euclid(16) as usize;
        section[(y * 16 + z) * 16 + x] = state;
    }

    pub(crate) fn set_block_entity(&mut self, pos: BlockPos, data: BlockEntityData) {
        self.block_entities.insert(pos, data);
    }

    fn section(&self, section_y: i32) -> Option<&[BlockStateId]> {
        self.sections.get(&section_y).map(Vec::as_slice)
    }

    fn heightmap(&self) -> [u32; 256] {
        let mut heights = [0u32; 256];
        for (section_y, states) in &self.sections {
            for (index, state) in states.iter().enumerate() {
                if state.0 == 0 {
                    continue;
                }
                let x = index % 16;
                let z = index / 16 % 16;
                let local_y = index / 256;
                let y = section_y * 16 + local_y as i32;
                let height = (y + 1 - MIN_Y) as u32;
                heights[x + z * 16] = heights[x + z * 16].max(height);
            }
        }
        heights
    }
}

pub(crate) fn encode_chunk(
    x: i32,
    z: i32,
    chunk: &ChunkSnapshot,
) -> Result<Vec<u8>, BlockEntityEncodeError> {
    let mut output = PacketBuf::new();
    output.write_i32(x);
    output.write_i32(z);
    write_heightmaps(&mut output, chunk);

    let mut section_data = PacketBuf::new();
    for section_index in 0..SECTION_COUNT {
        let section_y = MIN_SECTION_Y + section_index as i32;
        write_section(&mut section_data, chunk.section(section_y));
    }
    output.write_len(section_data.as_slice().len());
    output.write_bytes(section_data.as_slice());
    output.write_len(chunk.block_entities.len());
    for (pos, data) in &chunk.block_entities {
        write_chunk_block_entity(&mut output, *pos, data)?;
    }
    write_light_data(&mut output);
    Ok(output.into_inner())
}

fn write_section(output: &mut PacketBuf, states: Option<&[BlockStateId]>) {
    let air = [BlockStateId(0); 4_096];
    let states = states.unwrap_or(&air);
    let non_air = states.iter().filter(|state| state.0 != 0).count();
    output.write_u16(non_air as u16);
    output.write_u16(0);
    write_paletted_container(
        output,
        &states.iter().map(|state| state.0).collect::<Vec<_>>(),
        4,
        8,
        BLOCK_GLOBAL_BITS,
    );
    write_paletted_container(output, &[PLAINS_BIOME_ID; 64], 1, 3, 6);
}

fn write_paletted_container(
    output: &mut PacketBuf,
    values: &[u32],
    min_local_bits: u8,
    max_local_bits: u8,
    global_bits: u8,
) {
    let mut palette = Vec::new();
    let mut palette_ids = HashMap::<u32, u32>::new();
    let mut local_values = Vec::with_capacity(values.len());
    for value in values {
        let next_id = palette.len() as u32;
        let id = *palette_ids.entry(*value).or_insert_with(|| {
            palette.push(*value);
            next_id
        });
        local_values.push(id);
    }

    if palette.len() == 1 {
        output.write_u8(0);
        output.write_var_i32(palette[0] as i32);
        return;
    }

    let required_bits = ceil_log2(palette.len());
    if required_bits <= max_local_bits {
        let bits = required_bits.max(min_local_bits);
        output.write_u8(bits);
        output.write_len(palette.len());
        for value in palette {
            output.write_var_i32(value as i32);
        }
        write_storage(output, &local_values, bits);
    } else {
        output.write_u8(global_bits);
        write_storage(output, values, global_bits);
    }
}

fn write_storage(output: &mut PacketBuf, values: &[u32], bits: u8) {
    for value in pack_storage(values, bits) {
        output.write_u64(value);
    }
}

fn pack_storage(values: &[u32], bits: u8) -> Vec<u64> {
    let values_per_long = 64 / bits as usize;
    let mut storage = vec![0u64; values.len().div_ceil(values_per_long)];
    for (index, value) in values.iter().enumerate() {
        let cell = index / values_per_long;
        let offset = index % values_per_long * bits as usize;
        storage[cell] |= u64::from(*value) << offset;
    }
    storage
}

fn ceil_log2(value: usize) -> u8 {
    usize::BITS.saturating_sub((value - 1).leading_zeros()) as u8
}

fn write_heightmaps(output: &mut PacketBuf, chunk: &ChunkSnapshot) {
    let heights = chunk.heightmap();
    let storage = pack_storage(&heights, HEIGHTMAP_BITS);
    output.write_var_i32(2);
    for kind in [1, 4] {
        output.write_var_i32(kind);
        output.write_len(storage.len());
        for value in &storage {
            output.write_u64(*value);
        }
    }
}

fn write_light_data(output: &mut PacketBuf) {
    let mask = (1u64 << LIGHT_SECTION_COUNT) - 1;
    write_bitset(output, &[mask]);
    write_bitset(output, &[]);
    write_bitset(output, &[]);
    write_bitset(output, &[mask]);
    output.write_len(LIGHT_SECTION_COUNT);
    for _ in 0..LIGHT_SECTION_COUNT {
        output.write_len(LIGHT_BYTES);
        output.write_bytes(&[0xff; LIGHT_BYTES]);
    }
    output.write_var_i32(0);
}

fn write_bitset(output: &mut PacketBuf, values: &[u64]) {
    output.write_len(values.len());
    for value in values {
        output.write_u64(*value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_switches_at_local_thresholds() {
        assert_eq!(encoded_bits(1), 0);
        assert_eq!(encoded_bits(2), 4);
        assert_eq!(encoded_bits(16), 4);
        assert_eq!(encoded_bits(17), 5);
        assert_eq!(encoded_bits(256), 8);
        assert_eq!(encoded_bits(257), BLOCK_GLOBAL_BITS);
    }

    #[test]
    fn chunk_uses_negative_chunk_coordinates_and_complete_light() {
        let mut chunk = ChunkSnapshot::default();
        chunk.set_block(BlockPos::new(-17, -64, -1), BlockStateId(1));
        let encoded = encode_chunk(-2, -1, &chunk).unwrap();
        assert_eq!(&encoded[0..4], &(-2i32).to_be_bytes());
        assert_eq!(&encoded[4..8], &(-1i32).to_be_bytes());
        assert_eq!(encoded[8], 2);
        assert!(
            encoded
                .windows(LIGHT_BYTES)
                .any(|window| window.iter().all(|byte| *byte == 0xff))
        );
    }

    #[test]
    fn chunk_contains_network_block_entities() {
        let mut chunk = ChunkSnapshot::default();
        chunk.set_block_entity(
            BlockPos::new(-17, 12, -1),
            BlockEntityData {
                kind: "minecraft:sign".to_owned(),
                fields: BTreeMap::from([(
                    "front_text".to_owned(),
                    serde_json::json!({
                        "messages": ["one", "two", "three", "four"],
                        "filtered_messages": ["one", "two", "three", "four"],
                        "color": "black",
                        "has_glowing_text": false,
                    }),
                )]),
            },
        );
        let encoded = encode_chunk(-2, -1, &chunk).unwrap();
        let mut offset = 8;
        let heightmap_count = read_var_int(&encoded, &mut offset);
        for _ in 0..heightmap_count {
            read_var_int(&encoded, &mut offset);
            let storage_length = read_var_int(&encoded, &mut offset) as usize;
            offset += storage_length * 8;
        }
        let section_length = read_var_int(&encoded, &mut offset) as usize;
        offset += section_length;

        assert_eq!(read_var_int(&encoded, &mut offset), 1);
        assert_eq!(encoded[offset], 0xff);
        offset += 1;
        assert_eq!(
            i16::from_be_bytes(encoded[offset..offset + 2].try_into().unwrap()),
            12
        );
        offset += 2;
        assert_eq!(read_var_int(&encoded, &mut offset), 7);
        assert_eq!(encoded[offset], 10);
        assert!(
            encoded[offset..]
                .windows(10)
                .any(|value| value == b"front_text")
        );
        assert!(
            encoded[offset..]
                .windows(9)
                .any(|value| value == b"back_text")
        );
    }

    fn encoded_bits(distinct: usize) -> u8 {
        let values = (0..4_096)
            .map(|index| (index % distinct) as u32)
            .collect::<Vec<_>>();
        let mut output = PacketBuf::new();
        write_paletted_container(&mut output, &values, 4, 8, BLOCK_GLOBAL_BITS);
        output.as_slice()[0]
    }

    fn read_var_int(bytes: &[u8], offset: &mut usize) -> i32 {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = bytes[*offset];
            *offset += 1;
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return value as i32;
            }
        }
        panic!("invalid var int")
    }
}
