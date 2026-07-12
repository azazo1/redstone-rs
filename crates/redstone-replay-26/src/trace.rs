use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use redstone_core::{
    BlockEntityChange, BlockEntityData, BlockPos, BlockStateId, Direction, SparseWorld, WorldDelta,
    WorldEvent,
};
use zip::ZipArchive;

use crate::{PROTOCOL_VERSION, ReplayError, timestamp_for_tick, validate_block};

pub const RENDER_TRACE_ENTRY: &str = "redstone/render-v2.bin";

const MAGIC: &[u8; 8] = b"RSTRACE\0";
const FORMAT_VERSION: u16 = 2;
const END_TIMESTAMP: i32 = i32::MIN;
const WALLTIME_OFFSET: u64 = 8 + 2 + 4;
const PISTON_UPSERT: u8 = 1;
const PISTON_REMOVE: u8 = 2;

pub(crate) struct RenderTraceWriter {
    output: Option<BufWriter<File>>,
}

impl RenderTraceWriter {
    pub(crate) fn new(
        path: PathBuf,
        world: &SparseWorld,
        initial_tick: u64,
    ) -> Result<Self, ReplayError> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut output = BufWriter::new(file);
        output.write_all(MAGIC)?;
        output.write_all(&FORMAT_VERSION.to_le_bytes())?;
        output.write_all(&PROTOCOL_VERSION.to_le_bytes())?;
        output.write_all(&0u64.to_le_bytes())?;
        output.write_all(&0u64.to_le_bytes())?;
        let block_count = u64::try_from(world.iter_blocks().count())
            .map_err(|_| ReplayError::RenderTraceTooLarge)?;
        output.write_all(&block_count.to_le_bytes())?;
        for (pos, state) in world.iter_blocks() {
            write_block(&mut output, pos, state)?;
        }
        let initial_pistons = world
            .block_entities()
            .filter(|(_, data)| data.kind == "minecraft:moving_piston")
            .map(|(pos, data)| piston_snapshot(*pos, data, initial_tick))
            .collect::<Result<Vec<_>, _>>()?;
        let piston_count = u64::try_from(initial_pistons.len())
            .map_err(|_| ReplayError::RenderTraceTooLarge)?;
        output.write_all(&piston_count.to_le_bytes())?;
        for piston in &initial_pistons {
            write_piston(&mut output, piston)?;
        }
        Ok(Self {
            output: Some(output),
        })
    }

    pub(crate) fn record_delta(
        &mut self,
        timestamp: i32,
        delta: &WorldDelta,
    ) -> Result<(), ReplayError> {
        let changes = delta
            .events
            .iter()
            .filter_map(|event| match event {
                WorldEvent::Block { change } => Some(change),
                WorldEvent::BlockEntity { .. } | WorldEvent::BlockEvent { .. } => None,
            })
            .collect::<Vec<_>>();
        let pistons = delta
            .events
            .iter()
            .filter_map(|event| match event {
                WorldEvent::BlockEntity { change } => piston_event(change, delta.tick.0).transpose(),
                WorldEvent::Block { .. } | WorldEvent::BlockEvent { .. } => None,
            })
            .collect::<Result<Vec<_>, _>>()?;
        if changes.is_empty() && pistons.is_empty() {
            return Ok(());
        }
        let output = self.output.as_mut().ok_or(ReplayError::AlreadyFinished)?;
        output.write_all(&timestamp.to_le_bytes())?;
        let count = u32::try_from(changes.len()).map_err(|_| ReplayError::RenderTraceTooLarge)?;
        output.write_all(&count.to_le_bytes())?;
        let piston_count = u32::try_from(pistons.len())
            .map_err(|_| ReplayError::RenderTraceTooLarge)?;
        output.write_all(&piston_count.to_le_bytes())?;
        for change in changes {
            write_block(output, change.pos, change.new_state)?;
        }
        for piston in &pistons {
            write_piston_event(output, piston)?;
        }
        Ok(())
    }

    pub(crate) fn finish(
        &mut self,
        simulation_walltime: Duration,
        recorded_ticks: u64,
    ) -> Result<File, ReplayError> {
        let mut output = self.output.take().ok_or(ReplayError::AlreadyFinished)?;
        output.write_all(&END_TIMESTAMP.to_le_bytes())?;
        output.write_all(&0u32.to_le_bytes())?;
        output.write_all(&0u32.to_le_bytes())?;
        output.flush()?;
        let walltime_nanos = u64::try_from(simulation_walltime.as_nanos())
            .map_err(|_| ReplayError::RenderTraceWalltimeOverflow)?;
        output.seek(SeekFrom::Start(WALLTIME_OFFSET))?;
        output.write_all(&walltime_nanos.to_le_bytes())?;
        output.write_all(&recorded_ticks.to_le_bytes())?;
        output.flush()?;
        let file = output.into_inner().map_err(|error| error.into_error())?;
        file.sync_all()?;
        Ok(file)
    }
}

fn write_block(
    output: &mut impl Write,
    pos: BlockPos,
    state: BlockStateId,
) -> Result<(), ReplayError> {
    output.write_all(&pos.x.to_le_bytes())?;
    output.write_all(&pos.y.to_le_bytes())?;
    output.write_all(&pos.z.to_le_bytes())?;
    output.write_all(&state.0.to_le_bytes())?;
    Ok(())
}

fn piston_event(
    change: &BlockEntityChange,
    tick: u64,
) -> Result<Option<RenderTracePistonEvent>, ReplayError> {
    match change {
        BlockEntityChange::Create { pos, data }
        | BlockEntityChange::Update {
            pos,
            new_data: data,
            ..
        } if data.kind == "minecraft:moving_piston" => Ok(Some(
            RenderTracePistonEvent::Upsert(piston_snapshot(*pos, data, tick)?),
        )),
        BlockEntityChange::Remove { pos, data }
            if data.kind == "minecraft:moving_piston" =>
        {
            Ok(Some(RenderTracePistonEvent::Remove { pos: *pos }))
        }
        _ => Ok(None),
    }
}

fn piston_snapshot(
    pos: BlockPos,
    data: &BlockEntityData,
    current_tick: u64,
) -> Result<RenderTracePiston, ReplayError> {
    let moved_state = data
        .fields
        .get("moved_state")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .map(BlockStateId)
        .ok_or(ReplayError::InvalidRenderPistonField {
            pos,
            field: "moved_state",
        })?;
    validate_block(pos, moved_state.0)?;
    let direction = data
        .fields
        .get("direction")
        .and_then(serde_json::Value::as_str)
        .and_then(parse_direction)
        .ok_or(ReplayError::InvalidRenderPistonField {
            pos,
            field: "direction",
        })?;
    let extending = data
        .fields
        .get("extending")
        .and_then(serde_json::Value::as_bool)
        .ok_or(ReplayError::InvalidRenderPistonField {
            pos,
            field: "extending",
        })?;
    let source = data
        .fields
        .get("source")
        .and_then(serde_json::Value::as_bool)
        .ok_or(ReplayError::InvalidRenderPistonField {
            pos,
            field: "source",
        })?;
    let settle_tick = data
        .fields
        .get("settle_tick")
        .and_then(serde_json::Value::as_u64)
        .ok_or(ReplayError::InvalidRenderPistonField {
            pos,
            field: "settle_tick",
        })?;
    if settle_tick < current_tick {
        return Err(ReplayError::InvalidRenderPistonInterval {
            pos,
            start_tick: current_tick,
            settle_tick,
        });
    }
    let start_tick = settle_tick.saturating_sub(2);
    Ok(RenderTracePiston {
        pos,
        moved_state,
        direction,
        extending,
        source,
        start_timestamp_ms: timestamp_for_tick(start_tick)?,
        settle_timestamp_ms: timestamp_for_tick(settle_tick)?,
    })
}

fn parse_direction(value: &str) -> Option<Direction> {
    match value {
        "down" => Some(Direction::Down),
        "up" => Some(Direction::Up),
        "north" => Some(Direction::North),
        "south" => Some(Direction::South),
        "west" => Some(Direction::West),
        "east" => Some(Direction::East),
        _ => None,
    }
}

fn direction_id(direction: Direction) -> u8 {
    match direction {
        Direction::Down => 0,
        Direction::Up => 1,
        Direction::North => 2,
        Direction::South => 3,
        Direction::West => 4,
        Direction::East => 5,
    }
}

fn direction_from_id(id: u8) -> Result<Direction, ReplayError> {
    match id {
        0 => Ok(Direction::Down),
        1 => Ok(Direction::Up),
        2 => Ok(Direction::North),
        3 => Ok(Direction::South),
        4 => Ok(Direction::West),
        5 => Ok(Direction::East),
        _ => Err(ReplayError::InvalidRenderPistonDirection { direction: id }),
    }
}

fn write_piston(output: &mut impl Write, piston: &RenderTracePiston) -> Result<(), ReplayError> {
    output.write_all(&piston.pos.x.to_le_bytes())?;
    output.write_all(&piston.pos.y.to_le_bytes())?;
    output.write_all(&piston.pos.z.to_le_bytes())?;
    output.write_all(&piston.moved_state.0.to_le_bytes())?;
    output.write_all(&[direction_id(piston.direction)])?;
    let flags = u8::from(piston.extending) | (u8::from(piston.source) << 1);
    output.write_all(&[flags])?;
    output.write_all(&piston.start_timestamp_ms.to_le_bytes())?;
    output.write_all(&piston.settle_timestamp_ms.to_le_bytes())?;
    Ok(())
}

fn write_piston_event(
    output: &mut impl Write,
    event: &RenderTracePistonEvent,
) -> Result<(), ReplayError> {
    match event {
        RenderTracePistonEvent::Upsert(piston) => {
            output.write_all(&[PISTON_UPSERT])?;
            write_piston(output, piston)?;
        }
        RenderTracePistonEvent::Remove { pos } => {
            output.write_all(&[PISTON_REMOVE])?;
            output.write_all(&pos.x.to_le_bytes())?;
            output.write_all(&pos.y.to_le_bytes())?;
            output.write_all(&pos.z.to_le_bytes())?;
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderTraceBlock {
    pub pos: BlockPos,
    pub state: BlockStateId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderTracePiston {
    pub pos: BlockPos,
    pub moved_state: BlockStateId,
    pub direction: Direction,
    pub extending: bool,
    pub source: bool,
    pub start_timestamp_ms: i32,
    pub settle_timestamp_ms: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderTracePistonEvent {
    Upsert(RenderTracePiston),
    Remove { pos: BlockPos },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTraceFrame {
    pub timestamp_ms: i32,
    pub changes: Vec<RenderTraceBlock>,
    pub pistons: Vec<RenderTracePistonEvent>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTrace {
    pub simulation_walltime: Duration,
    pub recorded_ticks: u64,
    pub initial_blocks: Vec<RenderTraceBlock>,
    pub initial_pistons: Vec<RenderTracePiston>,
    pub frames: Vec<RenderTraceFrame>,
}

impl RenderTrace {
    pub fn read_mcpr(path: impl AsRef<Path>) -> Result<Self, ReplayError> {
        let file = File::open(path)?;
        let mut archive = ZipArchive::new(file)?;
        let entry = archive
            .by_name(RENDER_TRACE_ENTRY)
            .map_err(|error| match error {
                zip::result::ZipError::FileNotFound => ReplayError::MissingRenderTrace,
                other => ReplayError::Zip(other),
            })?;
        Self::read(BufReader::new(entry))
    }

    pub fn read(mut input: impl Read) -> Result<Self, ReplayError> {
        let mut magic = [0; 8];
        input.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(ReplayError::InvalidRenderTraceMagic);
        }
        let version = read_u16(&mut input)?;
        if version != FORMAT_VERSION {
            return Err(ReplayError::UnsupportedRenderTraceVersion { version });
        }
        let protocol = read_i32(&mut input)?;
        if protocol != PROTOCOL_VERSION {
            return Err(ReplayError::RenderTraceProtocol {
                expected: PROTOCOL_VERSION,
                actual: protocol,
            });
        }
        let simulation_walltime = Duration::from_nanos(read_u64(&mut input)?);
        let recorded_ticks = read_u64(&mut input)?;
        let initial_count = read_u64(&mut input)?;
        let initial_capacity = usize::try_from(initial_count)
            .map_err(|_| ReplayError::RenderTraceTooLarge)?;
        let mut initial_blocks = Vec::with_capacity(initial_capacity);
        for _ in 0..initial_count {
            initial_blocks.push(read_block(&mut input)?);
        }
        let initial_piston_count = read_u64(&mut input)?;
        let initial_piston_capacity = usize::try_from(initial_piston_count)
            .map_err(|_| ReplayError::RenderTraceTooLarge)?;
        let mut initial_pistons = Vec::with_capacity(initial_piston_capacity);
        for _ in 0..initial_piston_count {
            initial_pistons.push(read_piston(&mut input)?);
        }
        let mut frames = Vec::new();
        let mut previous_timestamp = 0;
        loop {
            let timestamp_ms = read_i32(&mut input)?;
            let count = read_u32(&mut input)?;
            let piston_count = read_u32(&mut input)?;
            if timestamp_ms == END_TIMESTAMP {
                if count != 0 || piston_count != 0 {
                    return Err(ReplayError::InvalidRenderTraceEnd);
                }
                break;
            }
            if timestamp_ms < 0 {
                return Err(ReplayError::InvalidRenderTraceTimestamp { timestamp_ms });
            }
            if timestamp_ms < previous_timestamp {
                return Err(ReplayError::TimestampOrder {
                    previous: previous_timestamp,
                    next: timestamp_ms,
                });
            }
            previous_timestamp = timestamp_ms;
            let capacity = usize::try_from(count).map_err(|_| ReplayError::RenderTraceTooLarge)?;
            let mut changes = Vec::with_capacity(capacity);
            for _ in 0..count {
                changes.push(read_block(&mut input)?);
            }
            let piston_capacity = usize::try_from(piston_count)
                .map_err(|_| ReplayError::RenderTraceTooLarge)?;
            let mut pistons = Vec::with_capacity(piston_capacity);
            for _ in 0..piston_count {
                pistons.push(read_piston_event(&mut input)?);
            }
            frames.push(RenderTraceFrame {
                timestamp_ms,
                changes,
                pistons,
            });
        }
        Ok(Self {
            simulation_walltime,
            recorded_ticks,
            initial_blocks,
            initial_pistons,
            frames,
        })
    }
}

fn read_block(input: &mut impl Read) -> Result<RenderTraceBlock, ReplayError> {
    let block = RenderTraceBlock {
        pos: BlockPos::new(read_i32(input)?, read_i32(input)?, read_i32(input)?),
        state: BlockStateId(read_u32(input)?),
    };
    validate_block(block.pos, block.state.0)?;
    Ok(block)
}

fn read_piston(input: &mut impl Read) -> Result<RenderTracePiston, ReplayError> {
    let piston = RenderTracePiston {
        pos: BlockPos::new(read_i32(input)?, read_i32(input)?, read_i32(input)?),
        moved_state: BlockStateId(read_u32(input)?),
        direction: direction_from_id(read_u8(input)?)?,
        extending: false,
        source: false,
        start_timestamp_ms: 0,
        settle_timestamp_ms: 0,
    };
    let flags = read_u8(input)?;
    if flags & !0b11 != 0 {
        return Err(ReplayError::InvalidRenderPistonFlags { flags });
    }
    let piston = RenderTracePiston {
        extending: flags & 1 != 0,
        source: flags & 2 != 0,
        start_timestamp_ms: read_i32(input)?,
        settle_timestamp_ms: read_i32(input)?,
        ..piston
    };
    validate_block(piston.pos, piston.moved_state.0)?;
    if piston.start_timestamp_ms < 0
        || piston.settle_timestamp_ms <= piston.start_timestamp_ms
    {
        return Err(ReplayError::InvalidRenderPistonTimestamps {
            pos: piston.pos,
            start_timestamp_ms: piston.start_timestamp_ms,
            settle_timestamp_ms: piston.settle_timestamp_ms,
        });
    }
    Ok(piston)
}

fn read_piston_event(input: &mut impl Read) -> Result<RenderTracePistonEvent, ReplayError> {
    match read_u8(input)? {
        PISTON_UPSERT => Ok(RenderTracePistonEvent::Upsert(read_piston(input)?)),
        PISTON_REMOVE => {
            let pos = BlockPos::new(read_i32(input)?, read_i32(input)?, read_i32(input)?);
            validate_block(pos, 0)?;
            Ok(RenderTracePistonEvent::Remove { pos })
        }
        tag => Err(ReplayError::InvalidRenderPistonEvent { tag }),
    }
}

fn read_u16(input: &mut impl Read) -> Result<u16, ReplayError> {
    let mut bytes = [0; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u8(input: &mut impl Read) -> Result<u8, ReplayError> {
    let mut byte = [0; 1];
    input.read_exact(&mut byte)?;
    Ok(byte[0])
}

fn read_u32(input: &mut impl Read) -> Result<u32, ReplayError> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_i32(input: &mut impl Read) -> Result<i32, ReplayError> {
    let mut bytes = [0; 4];
    input.read_exact(&mut bytes)?;
    Ok(i32::from_le_bytes(bytes))
}

fn read_u64(input: &mut impl Read) -> Result<u64, ReplayError> {
    let mut bytes = [0; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;

    use super::*;

    #[test]
    fn empty_trace_header_round_trips_walltime() {
        let bytes = trace_bytes(FORMAT_VERSION, true);
        let trace = RenderTrace::read(Cursor::new(bytes)).unwrap();

        assert_eq!(trace.simulation_walltime, Duration::from_nanos(123));
        assert_eq!(trace.recorded_ticks, 7);
        assert!(trace.initial_blocks.is_empty());
        assert!(trace.initial_pistons.is_empty());
        assert!(trace.frames.is_empty());
    }

    #[test]
    fn trace_rejects_magic_version_and_truncation() {
        let mut invalid_magic = trace_bytes(FORMAT_VERSION, true);
        invalid_magic[0] = b'X';
        assert!(matches!(
            RenderTrace::read(Cursor::new(invalid_magic)),
            Err(ReplayError::InvalidRenderTraceMagic)
        ));
        assert!(matches!(
            RenderTrace::read(Cursor::new(trace_bytes(FORMAT_VERSION + 1, true))),
            Err(ReplayError::UnsupportedRenderTraceVersion { .. })
        ));
        assert!(matches!(
            RenderTrace::read(Cursor::new(trace_bytes(1, true))),
            Err(ReplayError::UnsupportedRenderTraceVersion { version: 1 })
        ));
        assert!(matches!(
            RenderTrace::read(Cursor::new(trace_bytes(FORMAT_VERSION, false))),
            Err(ReplayError::Io(_))
        ));
    }

    #[test]
    fn moving_piston_snapshot_round_trips_all_directions() {
        for direction in ["down", "up", "north", "south", "west", "east"] {
            let piston = piston_snapshot(
                BlockPos::new(1, 2, 3),
                &moving_piston(direction),
                10,
            )
            .unwrap();
            assert_eq!(piston.start_timestamp_ms, 500);
            assert_eq!(piston.settle_timestamp_ms, 600);

            let mut bytes = Vec::new();
            write_piston(&mut bytes, &piston).unwrap();
            assert_eq!(read_piston(&mut Cursor::new(bytes)).unwrap(), piston);
        }
    }

    #[test]
    fn moving_piston_event_and_field_errors_are_explicit() {
        let piston = piston_snapshot(BlockPos::ZERO, &moving_piston("east"), 10).unwrap();
        let events = [
            RenderTracePistonEvent::Upsert(piston),
            RenderTracePistonEvent::Remove {
                pos: BlockPos::new(4, 5, 6),
            },
        ];
        for event in events {
            let mut bytes = Vec::new();
            write_piston_event(&mut bytes, &event).unwrap();
            assert_eq!(read_piston_event(&mut Cursor::new(bytes)).unwrap(), event);
        }

        let mut invalid = moving_piston("east");
        invalid.fields.remove("settle_tick");
        assert!(matches!(
            piston_snapshot(BlockPos::ZERO, &invalid, 10),
            Err(ReplayError::InvalidRenderPistonField {
                field: "settle_tick",
                ..
            })
        ));
        assert!(matches!(
            read_piston_event(&mut Cursor::new(vec![9])),
            Err(ReplayError::InvalidRenderPistonEvent { tag: 9 })
        ));
    }

    fn moving_piston(direction: &str) -> BlockEntityData {
        BlockEntityData {
            kind: "minecraft:moving_piston".to_owned(),
            fields: BTreeMap::from([
                ("moved_state".to_owned(), serde_json::Value::from(1)),
                (
                    "direction".to_owned(),
                    serde_json::Value::from(direction),
                ),
                ("extending".to_owned(), serde_json::Value::from(true)),
                ("source".to_owned(), serde_json::Value::from(false)),
                ("settle_tick".to_owned(), serde_json::Value::from(12)),
            ]),
        }
    }

    fn trace_bytes(version: u16, complete: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&123u64.to_le_bytes());
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        if complete {
            bytes.extend_from_slice(&END_TIMESTAMP.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes
    }
}
