use redstone_core::BlockPos;

#[derive(Clone, Debug, Default)]
pub(crate) struct PacketBuf {
    bytes: Vec<u8>,
}

impl PacketBuf {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn into_inner(self) -> Vec<u8> {
        self.bytes
    }

    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }

    pub(crate) fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    pub(crate) fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    pub(crate) fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_i32(&mut self, value: i32) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_i64(&mut self, value: i64) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_f32(&mut self, value: f32) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_f64(&mut self, value: f64) {
        self.write_bytes(&value.to_be_bytes());
    }

    pub(crate) fn write_var_i32(&mut self, value: i32) {
        let mut remaining = value as u32;
        loop {
            if remaining & !0x7f == 0 {
                self.write_u8(remaining as u8);
                return;
            }
            self.write_u8((remaining as u8 & 0x7f) | 0x80);
            remaining >>= 7;
        }
    }

    #[cfg(test)]
    pub(crate) fn write_var_i64(&mut self, value: i64) {
        let mut remaining = value as u64;
        loop {
            if remaining & !0x7f == 0 {
                self.write_u8(remaining as u8);
                return;
            }
            self.write_u8((remaining as u8 & 0x7f) | 0x80);
            remaining >>= 7;
        }
    }

    pub(crate) fn write_len(&mut self, value: usize) {
        self.write_var_i32(i32::try_from(value).expect("protocol length must fit in i32"));
    }

    pub(crate) fn write_string(&mut self, value: &str) {
        self.write_len(value.len());
        self.write_bytes(value.as_bytes());
    }

    pub(crate) fn write_identifier(&mut self, value: &str) {
        self.write_string(value);
    }

    pub(crate) fn write_block_pos(&mut self, pos: BlockPos) {
        self.write_u64(pack_block_pos(pos));
    }

}

pub(crate) fn pack_block_pos(pos: BlockPos) -> u64 {
    ((pos.x as u64 & 0x3ff_ffff) << 38)
        | ((pos.z as u64 & 0x3ff_ffff) << 12)
        | (pos.y as u64 & 0xfff)
}

#[cfg(test)]
pub(crate) fn pack_section_pos(x: i32, y: i32, z: i32) -> u64 {
    ((x as u64 & 0x3f_ffff) << 42)
        | ((z as u64 & 0x3f_ffff) << 20)
        | (y as u64 & 0xf_ffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn var_int_and_var_long_use_minecraft_twos_complement_encoding() {
        let mut buffer = PacketBuf::new();
        buffer.write_var_i32(-1);
        assert_eq!(buffer.as_slice(), &[0xff, 0xff, 0xff, 0xff, 0x0f]);

        let mut buffer = PacketBuf::new();
        buffer.write_var_i64(-1);
        assert_eq!(
            buffer.as_slice(),
            &[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]
        );
    }

    #[test]
    fn block_and_section_positions_preserve_negative_coordinates() {
        let block = BlockPos::new(-17, -64, 31);
        let packed = pack_block_pos(block);
        assert_eq!(sign_extend((packed >> 38) & 0x3ff_ffff, 26), -17);
        assert_eq!(sign_extend(packed & 0xfff, 12), -64);
        assert_eq!(sign_extend((packed >> 12) & 0x3ff_ffff, 26), 31);

        let packed = pack_section_pos(-2, -4, 1);
        assert_eq!(sign_extend((packed >> 42) & 0x3f_ffff, 22), -2);
        assert_eq!(sign_extend(packed & 0xf_ffff, 20), -4);
        assert_eq!(sign_extend((packed >> 20) & 0x3f_ffff, 22), 1);
    }

    fn sign_extend(value: u64, bits: u32) -> i64 {
        ((value << (64 - bits)) as i64) >> (64 - bits)
    }
}
