use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crc32fast::Hasher;
use redstone_core::{BlockPos, SparseWorld, WorldDelta};
use serde::Serialize;
use thiserror::Error;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

use crate::protocol::PacketState;
use crate::protocol::chunk::{
    ChunkSnapshot, MAX_BLOCK_STATE_ID, MAX_Y, MIN_Y, encode_chunk,
};
use crate::protocol::packets::{
    Camera, CONFIG_ENABLED_FEATURES, CONFIG_FINISH, CONFIG_REGISTRY_DATA,
    CONFIG_SELECT_KNOWN_PACKS, CONFIG_UPDATE_TAGS, LOGIN_FINISHED, PLAY_BLOCK_UPDATE,
    PLAY_CHUNK_BATCH_FINISHED, PLAY_CHUNK_BATCH_START, PLAY_LEVEL_CHUNK_WITH_LIGHT, PLAY_LOGIN,
    PLAY_PLAYER_POSITION, PLAY_SET_CHUNK_CACHE_CENTER, PLAY_SET_CHUNK_CACHE_RADIUS,
    PLAY_SET_DEFAULT_SPAWN, PLAY_SET_SIMULATION_DISTANCE, PLAY_SET_TIME, block_update,
    chunk_cache_center, default_spawn, enabled_features, login_finished, play_login,
    player_position, select_known_packs, set_time, single_var_int,
};
use crate::protocol::registry::{registry_packets, required_tags_packet};

mod protocol;

pub const MINECRAFT_VERSION: &str = "26.1.2";
pub const PROTOCOL_VERSION: i32 = 775;
pub const FILE_FORMAT_VERSION: i32 = 14;
const TICK_MILLIS: u64 = 50;
const MIN_VIEW_DISTANCE: i32 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplayRegion {
    pub min: BlockPos,
    pub max: BlockPos,
}

impl ReplayRegion {
    pub fn new(first: BlockPos, second: BlockPos) -> Self {
        Self {
            min: BlockPos::new(
                first.x.min(second.x),
                first.y.min(second.y),
                first.z.min(second.z),
            ),
            max: BlockPos::new(
                first.x.max(second.x),
                first.y.max(second.y),
                first.z.max(second.z),
            ),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplayOptions {
    pub name: String,
    pub seed: u64,
    pub experimental: bool,
    pub recorded_at: SystemTime,
    pub region: ReplayRegion,
}

impl ReplayOptions {
    pub fn new(
        name: impl Into<String>,
        seed: u64,
        experimental: bool,
        region: ReplayRegion,
    ) -> Self {
        Self {
            name: name.into(),
            seed,
            experimental,
            recorded_at: SystemTime::now(),
            region,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ReplayStats {
    pub ticks: u64,
    pub packets: u64,
    pub block_updates: u64,
    pub file_size: u64,
    pub elapsed: Duration,
}

pub struct ReplayWriter {
    output_path: PathBuf,
    recording_path: PathBuf,
    archive_path: PathBuf,
    recording: Option<BufWriter<File>>,
    archive: Option<File>,
    crc: Hasher,
    state: PacketState,
    loaded_chunks: BTreeSet<(i32, i32)>,
    chunk_cache_center: (i32, i32),
    chunk_radius: i32,
    packet_count: u64,
    block_updates: u64,
    last_tick: u64,
    last_timestamp: i32,
    options: ReplayOptions,
    started: Instant,
    finished: bool,
}

impl ReplayWriter {
    pub fn new(
        output_path: impl AsRef<Path>,
        options: ReplayOptions,
        initial_world: &SparseWorld,
    ) -> Result<Self, ReplayError> {
        let output_path = output_path.as_ref().to_path_buf();
        let file_name = output_path
            .file_name()
            .ok_or(ReplayError::MissingOutputName)?
            .to_string_lossy();
        let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let prefix = format!(".{file_name}.redstone-{}-{nonce}", std::process::id());
        let recording_path = parent.join(format!("{prefix}.tmcpr.tmp"));
        let archive_path = parent.join(format!("{prefix}.mcpr.tmp"));
        let recording_file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&recording_path)?;
        let archive_file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&archive_path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = std::fs::remove_file(&recording_path);
                return Err(error.into());
            }
        };
        let mut writer = Self {
            output_path,
            recording_path,
            archive_path,
            recording: Some(BufWriter::new(recording_file)),
            archive: Some(archive_file),
            crc: Hasher::new(),
            state: PacketState::Login,
            loaded_chunks: BTreeSet::new(),
            chunk_cache_center: (0, 0),
            chunk_radius: MIN_VIEW_DISTANCE,
            packet_count: 0,
            block_updates: 0,
            last_tick: 0,
            last_timestamp: 0,
            options,
            started: Instant::now(),
            finished: false,
        };
        writer.write_initial_world(initial_world)?;
        Ok(writer)
    }

    pub fn record_delta(&mut self, delta: &WorldDelta) -> Result<(), ReplayError> {
        if delta.tick.0 < self.last_tick {
            return Err(ReplayError::TickOrder {
                previous: self.last_tick,
                next: delta.tick.0,
            });
        }
        let timestamp = timestamp_for_tick(delta.tick.0)?;
        for change in &delta.changes {
            validate_block(change.pos, change.new_state.0)?;
            let chunk = chunk_pos(change.pos);
            self.ensure_chunk_area(timestamp, chunk)?;
            self.write_packet(
                timestamp,
                PacketState::Play,
                PLAY_BLOCK_UPDATE,
                &block_update(change.pos, change.new_state.0),
            )?;
            self.block_updates += 1;
        }
        self.last_tick = delta.tick.0;
        self.last_timestamp = timestamp;
        Ok(())
    }

    pub fn finish(mut self) -> Result<ReplayStats, ReplayError> {
        let mut recording = self.recording.take().ok_or(ReplayError::AlreadyFinished)?;
        recording.flush()?;
        let recording_file = recording.into_inner().map_err(|error| error.into_error())?;
        recording_file.sync_all()?;

        let archive_file = self.archive.take().ok_or(ReplayError::AlreadyFinished)?;
        let mut archive = ZipWriter::new(archive_file);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        archive.start_file("recording.tmcpr", options)?;
        let mut recording_input = File::open(&self.recording_path)?;
        std::io::copy(&mut recording_input, &mut archive)?;
        archive.start_file("recording.tmcpr.crc32", options)?;
        write!(archive, "{}", self.crc.clone().finalize())?;
        archive.start_file("metaData.json", options)?;
        let metadata = ReplayMetadata::new(&self.options, self.last_timestamp)?;
        serde_json::to_writer(&mut archive, &metadata)?;
        let archive_file = archive.finish()?;
        archive_file.sync_all()?;
        let file_size = archive_file.metadata()?.len();
        drop(archive_file);

        std::fs::rename(&self.archive_path, &self.output_path)?;
        let _ = std::fs::remove_file(&self.recording_path);
        self.finished = true;
        Ok(ReplayStats {
            ticks: self.last_tick,
            packets: self.packet_count,
            block_updates: self.block_updates,
            file_size,
            elapsed: self.started.elapsed(),
        })
    }

    fn write_initial_world(&mut self, world: &SparseWorld) -> Result<(), ReplayError> {
        let mut chunks = BTreeMap::<(i32, i32), ChunkSnapshot>::new();
        for (pos, state) in world.iter_blocks() {
            validate_block(pos, state.0)?;
            chunks.entry(chunk_pos(pos)).or_default().set_block(pos, state);
        }
        let region = ReplayRegion::new(self.options.region.min, self.options.region.max);
        validate_position(region.min)?;
        validate_position(region.max)?;
        let source_chunks = ChunkArea::from_region(region);
        let mut render_chunks = source_chunks.expanded(1).chunks().collect::<BTreeSet<_>>();
        let occupied_chunks = chunks.keys().copied().collect::<Vec<_>>();
        for occupied_chunk in occupied_chunks {
            render_chunks.extend(ChunkArea::around(occupied_chunk).expanded(1).chunks());
        }
        for chunk in render_chunks {
            chunks.entry(chunk).or_default();
        }

        let camera = camera_for_region(region);
        let camera_chunk = (
            floor_to_i32(camera.position[0]).div_euclid(16),
            floor_to_i32(camera.position[2]).div_euclid(16),
        );
        let chunk_radius = chunks
            .keys()
            .map(|(x, z)| (x - camera_chunk.0).abs().max((z - camera_chunk.1).abs()))
            .max()
            .unwrap_or(MIN_VIEW_DISTANCE)
            .max(MIN_VIEW_DISTANCE);
        self.chunk_cache_center = camera_chunk;
        self.chunk_radius = chunk_radius;

        self.write_packet(0, PacketState::Login, LOGIN_FINISHED, &login_finished())?;
        self.state = PacketState::Configuration;
        self.write_packet(
            0,
            PacketState::Configuration,
            CONFIG_SELECT_KNOWN_PACKS,
            &select_known_packs(),
        )?;
        for registry in registry_packets() {
            self.write_packet(
                0,
                PacketState::Configuration,
                CONFIG_REGISTRY_DATA,
                &registry,
            )?;
        }
        self.write_packet(
            0,
            PacketState::Configuration,
            CONFIG_ENABLED_FEATURES,
            &enabled_features(self.options.experimental),
        )?;
        self.write_packet(
            0,
            PacketState::Configuration,
            CONFIG_UPDATE_TAGS,
            &required_tags_packet(),
        )?;
        self.write_packet(0, PacketState::Configuration, CONFIG_FINISH, &[])?;
        self.state = PacketState::Play;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_LOGIN,
            &play_login(self.options.seed, chunk_radius),
        )?;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_SET_CHUNK_CACHE_CENTER,
            &chunk_cache_center(camera_chunk.0, camera_chunk.1),
        )?;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_SET_CHUNK_CACHE_RADIUS,
            &single_var_int(chunk_radius),
        )?;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_SET_SIMULATION_DISTANCE,
            &single_var_int(chunk_radius),
        )?;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_SET_DEFAULT_SPAWN,
            &default_spawn(camera),
        )?;
        self.write_packet(0, PacketState::Play, PLAY_SET_TIME, &set_time())?;
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_PLAYER_POSITION,
            &player_position(camera),
        )?;
        self.write_packet(0, PacketState::Play, PLAY_CHUNK_BATCH_START, &[])?;
        for ((x, z), chunk) in chunks {
            self.write_packet(
                0,
                PacketState::Play,
                PLAY_LEVEL_CHUNK_WITH_LIGHT,
                &encode_chunk(x, z, &chunk),
            )?;
            self.loaded_chunks.insert((x, z));
        }
        self.write_packet(
            0,
            PacketState::Play,
            PLAY_CHUNK_BATCH_FINISHED,
            &single_var_int(self.loaded_chunks.len() as i32),
        )?;
        Ok(())
    }

    fn ensure_chunk_area(
        &mut self,
        timestamp: i32,
        required_chunk: (i32, i32),
    ) -> Result<(), ReplayError> {
        let missing = ChunkArea::around(required_chunk)
            .expanded(1)
            .chunks()
            .filter(|chunk| !self.loaded_chunks.contains(chunk))
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(());
        }

        let required_radius = missing
            .iter()
            .copied()
            .map(|(x, z)| {
                (x - self.chunk_cache_center.0)
                    .abs()
                    .max((z - self.chunk_cache_center.1).abs())
            })
            .max()
            .unwrap_or(self.chunk_radius)
            .max(MIN_VIEW_DISTANCE);
        if required_radius > self.chunk_radius {
            self.write_packet(
                timestamp,
                PacketState::Play,
                PLAY_SET_CHUNK_CACHE_RADIUS,
                &single_var_int(required_radius),
            )?;
            self.chunk_radius = required_radius;
        }

        self.write_packet(timestamp, PacketState::Play, PLAY_CHUNK_BATCH_START, &[])?;
        for chunk in &missing {
            self.write_packet(
                timestamp,
                PacketState::Play,
                PLAY_LEVEL_CHUNK_WITH_LIGHT,
                &encode_chunk(chunk.0, chunk.1, &ChunkSnapshot::default()),
            )?;
            self.loaded_chunks.insert(*chunk);
        }
        self.write_packet(
            timestamp,
            PacketState::Play,
            PLAY_CHUNK_BATCH_FINISHED,
            &single_var_int(missing.len() as i32),
        )?;
        Ok(())
    }

    fn write_packet(
        &mut self,
        timestamp: i32,
        state: PacketState,
        packet_id: i32,
        payload: &[u8],
    ) -> Result<(), ReplayError> {
        if self.state != state {
            return Err(ReplayError::ProtocolState {
                expected: state_name(self.state),
                actual: state_name(state),
            });
        }
        if timestamp < self.last_timestamp {
            return Err(ReplayError::TimestampOrder {
                previous: self.last_timestamp,
                next: timestamp,
            });
        }
        let mut packet = protocol::buf::PacketBuf::new();
        packet.write_var_i32(packet_id);
        packet.write_bytes(payload);
        let packet = packet.into_inner();
        let packet_len = i32::try_from(packet.len()).map_err(|_| ReplayError::PacketTooLarge)?;
        let recording = self.recording.as_mut().ok_or(ReplayError::AlreadyFinished)?;
        let timestamp_bytes = timestamp.to_be_bytes();
        let length_bytes = packet_len.to_be_bytes();
        recording.write_all(&timestamp_bytes)?;
        recording.write_all(&length_bytes)?;
        recording.write_all(&packet)?;
        self.crc.update(&timestamp_bytes);
        self.crc.update(&length_bytes);
        self.crc.update(&packet);
        self.packet_count += 1;
        self.last_timestamp = timestamp;
        Ok(())
    }
}

impl Drop for ReplayWriter {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let _ = std::fs::remove_file(&self.recording_path);
        let _ = std::fs::remove_file(&self.archive_path);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReplayMetadata<'a> {
    singleplayer: bool,
    server_name: &'a str,
    custom_server_name: &'a str,
    duration: i32,
    date: u64,
    mcversion: &'static str,
    file_format: &'static str,
    file_format_version: i32,
    protocol: i32,
    generator: String,
    self_id: i32,
    players: Vec<String>,
}

impl<'a> ReplayMetadata<'a> {
    fn new(options: &'a ReplayOptions, duration: i32) -> Result<Self, ReplayError> {
        let date = options
            .recorded_at
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ReplayError::InvalidRecordingDate)?
            .as_millis();
        Ok(Self {
            singleplayer: true,
            server_name: &options.name,
            custom_server_name: &options.name,
            duration,
            date: u64::try_from(date).map_err(|_| ReplayError::InvalidRecordingDate)?,
            mcversion: MINECRAFT_VERSION,
            file_format: "MCPR",
            file_format_version: FILE_FORMAT_VERSION,
            protocol: PROTOCOL_VERSION,
            generator: format!("redstone-rs {}", env!("CARGO_PKG_VERSION")),
            self_id: -1,
            players: Vec::new(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ChunkArea {
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
}

impl ChunkArea {
    fn from_region(region: ReplayRegion) -> Self {
        Self {
            min_x: region.min.x.div_euclid(16),
            max_x: region.max.x.div_euclid(16),
            min_z: region.min.z.div_euclid(16),
            max_z: region.max.z.div_euclid(16),
        }
    }

    fn around(chunk: (i32, i32)) -> Self {
        Self {
            min_x: chunk.0,
            max_x: chunk.0,
            min_z: chunk.1,
            max_z: chunk.1,
        }
    }

    fn expanded(self, chunks: i32) -> Self {
        Self {
            min_x: self.min_x.saturating_sub(chunks),
            max_x: self.max_x.saturating_add(chunks),
            min_z: self.min_z.saturating_sub(chunks),
            max_z: self.max_z.saturating_add(chunks),
        }
    }

    fn chunks(self) -> impl Iterator<Item = (i32, i32)> {
        (self.min_x..=self.max_x)
            .flat_map(move |x| (self.min_z..=self.max_z).map(move |z| (x, z)))
    }
}

fn camera_for_region(region: ReplayRegion) -> Camera {
    let target = [
        (f64::from(region.min.x) + f64::from(region.max.x) + 1.0) / 2.0,
        (f64::from(region.min.y) + f64::from(region.max.y) + 1.0) / 2.0,
        (f64::from(region.min.z) + f64::from(region.max.z) + 1.0) / 2.0,
    ];
    let width = (f64::from(region.max.x) - f64::from(region.min.x) + 1.0).max(1.0);
    let height = (f64::from(region.max.y) - f64::from(region.min.y) + 1.0).max(1.0);
    let depth = (f64::from(region.max.z) - f64::from(region.min.z) + 1.0).max(1.0);
    let distance = width.max(depth).max(height).max(6.0) * 1.35;
    let horizontal_offset = distance.min(12.0);
    let source_chunks = ChunkArea::from_region(region);
    let min_camera_x = f64::from(source_chunks.min_x) * 16.0 + 0.5;
    let max_camera_x = f64::from(source_chunks.max_x.saturating_add(1)) * 16.0 - 0.5;
    let min_camera_z = f64::from(source_chunks.min_z) * 16.0 + 0.5;
    let max_camera_z = f64::from(source_chunks.max_z.saturating_add(1)) * 16.0 - 0.5;
    let position = [
        (target[0] + horizontal_offset).clamp(min_camera_x, max_camera_x),
        (f64::from(region.max.y) + distance * 0.75 + 3.0).min(318.0),
        (target[2] + horizontal_offset).clamp(min_camera_z, max_camera_z),
    ];
    camera_looking_at(position, target)
}

fn camera_looking_at(position: [f64; 3], target: [f64; 3]) -> Camera {
    let dx = target[0] - position[0];
    let dy = target[1] - position[1];
    let dz = target[2] - position[2];
    let horizontal = dx.hypot(dz);
    Camera {
        position,
        yaw: (-dx).atan2(dz).to_degrees() as f32,
        pitch: (-dy.atan2(horizontal)).to_degrees() as f32,
        target: BlockPos::new(
            floor_to_i32(target[0]),
            floor_to_i32(target[1]).clamp(MIN_Y, MAX_Y),
            floor_to_i32(target[2]),
        ),
    }
}

fn floor_to_i32(value: f64) -> i32 {
    value.floor().clamp(i32::MIN as f64, i32::MAX as f64) as i32
}

fn chunk_pos(pos: BlockPos) -> (i32, i32) {
    (pos.x.div_euclid(16), pos.z.div_euclid(16))
}

fn validate_block(pos: BlockPos, state: u32) -> Result<(), ReplayError> {
    validate_position(pos)?;
    if state > MAX_BLOCK_STATE_ID {
        return Err(ReplayError::BlockStateOutOfRange { state });
    }
    Ok(())
}

fn validate_position(pos: BlockPos) -> Result<(), ReplayError> {
    if !(MIN_Y..=MAX_Y).contains(&pos.y) {
        return Err(ReplayError::HeightOutOfRange { pos });
    }
    if !(-33_554_432..=33_554_431).contains(&pos.x)
        || !(-33_554_432..=33_554_431).contains(&pos.z)
    {
        return Err(ReplayError::PositionOutOfRange { pos });
    }
    Ok(())
}

fn timestamp_for_tick(tick: u64) -> Result<i32, ReplayError> {
    let timestamp = tick
        .checked_mul(TICK_MILLIS)
        .ok_or(ReplayError::TimestampOverflow { tick })?;
    i32::try_from(timestamp).map_err(|_| ReplayError::TimestampOverflow { tick })
}

fn state_name(state: PacketState) -> &'static str {
    match state {
        PacketState::Login => "login",
        PacketState::Configuration => "configuration",
        PacketState::Play => "play",
    }
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("录像输出路径缺少文件名")]
    MissingOutputName,
    #[error("录像时间早于 Unix epoch")]
    InvalidRecordingDate,
    #[error("方块坐标超出 overworld 高度范围: {pos:?}, 允许 Y={MIN_Y}..{MAX_Y}")]
    HeightOutOfRange { pos: BlockPos },
    #[error("方块坐标超出 Minecraft 网络坐标范围: {pos:?}")]
    PositionOutOfRange { pos: BlockPos },
    #[error("方块状态 ID 超出 26.1.2 全局注册表范围: {state}")]
    BlockStateOutOfRange { state: u32 },
    #[error("tick {tick} 无法转换为 MCPR 毫秒时间戳")]
    TimestampOverflow { tick: u64 },
    #[error("录像时间戳倒退: {previous} -> {next}")]
    TimestampOrder { previous: i32, next: i32 },
    #[error("录像 tick 倒退: {previous} -> {next}")]
    TickOrder { previous: u64, next: u64 },
    #[error("协议状态错误: 需要 {expected:?}, 收到 {actual:?}")]
    ProtocolState {
        expected: &'static str,
        actual: &'static str,
    },
    #[error("单个网络包过大")]
    PacketTooLarge,
    #[error("录像 writer 已经完成")]
    AlreadyFinished,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use redstone_core::{BlockChange, BlockStateId, GameTick};
    use zip::ZipArchive;

    use super::*;

    #[test]
    fn timestamp_encoding_maps_each_tick_to_fifty_milliseconds() {
        assert_eq!(timestamp_for_tick(0).unwrap(), 0);
        assert_eq!(timestamp_for_tick(1).unwrap(), 50);
        assert_eq!(timestamp_for_tick(20).unwrap(), 1_000);
    }

    #[test]
    fn replay_contains_metadata_crc_and_ordered_state_transitions() {
        let directory = TestDirectory::new();
        let output = directory.path.join("sample.mcpr");
        let mut world = SparseWorld::new(BlockStateId(0));
        world
            .set_block(BlockPos::new(-1, -64, 16), BlockStateId(1))
            .unwrap();
        let mut writer = ReplayWriter::new(
            &output,
            ReplayOptions {
                name: "sample.toml".to_owned(),
                seed: 7,
                experimental: false,
                recorded_at: UNIX_EPOCH + Duration::from_millis(1234),
                region: ReplayRegion::new(
                    BlockPos::new(-1, -64, 16),
                    BlockPos::new(-1, -64, 16),
                ),
            },
            &world,
        )
        .unwrap();
        writer
            .record_delta(&WorldDelta {
                tick: GameTick(2),
                changes: vec![BlockChange {
                    pos: BlockPos::new(32, 0, -1),
                    old_state: BlockStateId(0),
                    new_state: BlockStateId(1),
                }],
                probes: Vec::new(),
            })
            .unwrap();
        writer
            .record_delta(&WorldDelta {
                tick: GameTick(3),
                changes: Vec::new(),
                probes: Vec::new(),
            })
            .unwrap();
        let stats = writer.finish().unwrap();
        assert_eq!(stats.ticks, 3);
        assert_eq!(stats.block_updates, 1);

        let mut archive = ZipArchive::new(File::open(output).unwrap()).unwrap();
        assert!(archive.by_name("recording.tmcpr").is_ok());
        assert!(archive.by_name("recording.tmcpr.crc32").is_ok());
        let metadata = {
            let mut entry = archive.by_name("metaData.json").unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
        };
        assert_eq!(metadata["fileFormatVersion"], FILE_FORMAT_VERSION);
        assert_eq!(metadata["protocol"], PROTOCOL_VERSION);
        assert_eq!(metadata["duration"], 150);
        assert_eq!(metadata["date"], 1234);

        let recording = read_entry(&mut archive, "recording.tmcpr");
        let crc = String::from_utf8(read_entry(&mut archive, "recording.tmcpr.crc32")).unwrap();
        assert_eq!(crc, crc32fast::hash(&recording).to_string());
        let packets = parse_packets(&recording);
        assert_eq!(packets[0].1, LOGIN_FINISHED);
        assert_eq!(packets[1].1, CONFIG_SELECT_KNOWN_PACKS);
        assert!(packets.iter().any(|packet| packet.1 == CONFIG_FINISH));
        assert!(packets.iter().any(|packet| packet.1 == PLAY_LOGIN));
        assert_eq!(packets.last().unwrap().0, 100);
        assert_eq!(packets.last().unwrap().1, PLAY_BLOCK_UPDATE);
    }

    #[test]
    fn encoding_error_removes_temporary_files_and_target() {
        let directory = TestDirectory::new();
        let output = directory.path.join("invalid.mcpr");
        let world = SparseWorld::new(BlockStateId(0));
        let mut writer = ReplayWriter::new(
            &output,
            ReplayOptions::new(
                "invalid.toml",
                0,
                false,
                ReplayRegion::new(BlockPos::ZERO, BlockPos::ZERO),
            ),
            &world,
        )
        .unwrap();
        let error = writer.record_delta(&WorldDelta {
            tick: GameTick(1),
            changes: vec![BlockChange {
                pos: BlockPos::new(0, MAX_Y + 1, 0),
                old_state: BlockStateId(0),
                new_state: BlockStateId(1),
            }],
            probes: Vec::new(),
        });
        assert!(matches!(error, Err(ReplayError::HeightOutOfRange { .. })));
        drop(writer);
        assert!(!output.exists());
        assert_eq!(std::fs::read_dir(&directory.path).unwrap().count(), 0);
    }

    fn read_entry(archive: &mut ZipArchive<File>, name: &str) -> Vec<u8> {
        let mut entry = archive.by_name(name).unwrap();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        bytes
    }

    fn parse_packets(mut bytes: &[u8]) -> Vec<(i32, i32)> {
        let mut packets = Vec::new();
        while !bytes.is_empty() {
            let timestamp = i32::from_be_bytes(bytes[0..4].try_into().unwrap());
            let length = i32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
            let packet = &bytes[8..8 + length];
            let (packet_id, _) = read_var_int(packet);
            packets.push((timestamp, packet_id));
            bytes = &bytes[8 + length..];
        }
        packets
    }

    fn read_var_int(bytes: &[u8]) -> (i32, usize) {
        let mut value = 0u32;
        for (index, byte) in bytes.iter().enumerate() {
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return (value as i32, index + 1);
            }
        }
        panic!("invalid var int")
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "redstone-replay-test-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
