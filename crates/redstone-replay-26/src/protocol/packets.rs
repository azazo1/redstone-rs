use redstone_core::BlockPos;

use super::buf::PacketBuf;

pub(crate) const LOGIN_FINISHED: i32 = 2;
pub(crate) const CONFIG_FINISH: i32 = 3;
pub(crate) const CONFIG_REGISTRY_DATA: i32 = 7;
pub(crate) const CONFIG_ENABLED_FEATURES: i32 = 12;
pub(crate) const CONFIG_UPDATE_TAGS: i32 = 13;
pub(crate) const CONFIG_SELECT_KNOWN_PACKS: i32 = 14;
pub(crate) const PLAY_BLOCK_ENTITY_DATA: i32 = 6;
pub(crate) const PLAY_BLOCK_UPDATE: i32 = 8;
pub(crate) const PLAY_CHUNK_BATCH_FINISHED: i32 = 11;
pub(crate) const PLAY_CHUNK_BATCH_START: i32 = 12;
pub(crate) const PLAY_LEVEL_CHUNK_WITH_LIGHT: i32 = 45;
pub(crate) const PLAY_LOGIN: i32 = 49;
pub(crate) const PLAY_PLAYER_POSITION: i32 = 72;
pub(crate) const PLAY_SET_CHUNK_CACHE_CENTER: i32 = 94;
pub(crate) const PLAY_SET_CHUNK_CACHE_RADIUS: i32 = 95;
pub(crate) const PLAY_SET_DEFAULT_SPAWN: i32 = 97;
pub(crate) const PLAY_SET_SIMULATION_DISTANCE: i32 = 111;
pub(crate) const PLAY_SET_TIME: i32 = 113;

const OVERWORLD: &str = "minecraft:overworld";

#[derive(Clone, Copy, Debug)]
pub(crate) struct Camera {
    pub(crate) position: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) target: BlockPos,
}

pub(crate) fn login_finished() -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_bytes(&[
        0x6d, 0x63, 0x70, 0x72, 0x00, 0x00, 0x40, 0x00, 0x80, 0x00, 0x72, 0x65, 0x70, 0x6c,
        0x61, 0x79,
    ]);
    output.write_string("ReplayCamera");
    output.write_var_i32(0);
    output.into_inner()
}

pub(crate) fn select_known_packs() -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_var_i32(1);
    output.write_string("minecraft");
    output.write_string("core");
    output.write_string("26.1.2");
    output.into_inner()
}

pub(crate) fn enabled_features(experimental: bool) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_var_i32(if experimental { 2 } else { 1 });
    output.write_identifier("minecraft:vanilla");
    if experimental {
        output.write_identifier("minecraft:redstone_experiments");
    }
    output.into_inner()
}

pub(crate) fn play_login(seed: u64, chunk_radius: i32) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_i32(1);
    output.write_bool(false);
    output.write_var_i32(1);
    output.write_identifier(OVERWORLD);
    output.write_var_i32(1);
    output.write_var_i32(chunk_radius);
    output.write_var_i32(chunk_radius);
    output.write_bool(false);
    output.write_bool(true);
    output.write_bool(false);
    output.write_var_i32(0);
    output.write_identifier(OVERWORLD);
    output.write_i64(seed as i64);
    output.write_u8(3);
    output.write_u8(0xff);
    output.write_bool(false);
    output.write_bool(false);
    output.write_bool(false);
    output.write_var_i32(0);
    output.write_var_i32(63);
    output.write_bool(false);
    output.into_inner()
}

pub(crate) fn chunk_cache_center(x: i32, z: i32) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_var_i32(x);
    output.write_var_i32(z);
    output.into_inner()
}

pub(crate) fn single_var_int(value: i32) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_var_i32(value);
    output.into_inner()
}

pub(crate) fn default_spawn(camera: Camera) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_identifier(OVERWORLD);
    output.write_block_pos(camera.target);
    output.write_f32(camera.yaw);
    output.write_f32(camera.pitch);
    output.into_inner()
}

pub(crate) fn set_time() -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_i64(6_000);
    output.write_var_i32(0);
    output.into_inner()
}

pub(crate) fn player_position(camera: Camera) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_var_i32(1);
    for value in camera.position {
        output.write_f64(value);
    }
    for _ in 0..3 {
        output.write_f64(0.0);
    }
    output.write_f32(camera.yaw);
    output.write_f32(camera.pitch);
    output.write_i32(0);
    output.into_inner()
}

pub(crate) fn block_update(pos: BlockPos, state: u32) -> Vec<u8> {
    let mut output = PacketBuf::new();
    output.write_block_pos(pos);
    output.write_var_i32(state as i32);
    output.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_position_uses_absolute_relative_mask() {
        let encoded = player_position(Camera {
            position: [1.0, 2.0, 3.0],
            yaw: 4.0,
            pitch: 5.0,
            target: BlockPos::ZERO,
        });
        assert_eq!(&encoded[encoded.len() - 4..], &[0, 0, 0, 0]);
    }
}
