use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use redstone_core::{BlockPos, BlockStateId, SparseWorld, WorldDelta, WorldEvent};
use zip::ZipArchive;

use crate::{PROTOCOL_VERSION, ReplayError};

pub const RENDER_TRACE_ENTRY: &str = "redstone/render-v1.bin";

const MAGIC: &[u8; 8] = b"RSTRACE\0";
const FORMAT_VERSION: u16 = 1;
const END_TIMESTAMP: i32 = i32::MIN;
const WALLTIME_OFFSET: u64 = 8 + 2 + 4;

pub(crate) struct RenderTraceWriter {
    output: Option<BufWriter<File>>,
}

impl RenderTraceWriter {
    pub(crate) fn new(path: PathBuf, world: &SparseWorld) -> Result<Self, ReplayError> {
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
        if changes.is_empty() {
            return Ok(());
        }
        let output = self.output.as_mut().ok_or(ReplayError::AlreadyFinished)?;
        output.write_all(&timestamp.to_le_bytes())?;
        let count = u32::try_from(changes.len()).map_err(|_| ReplayError::RenderTraceTooLarge)?;
        output.write_all(&count.to_le_bytes())?;
        for change in changes {
            write_block(output, change.pos, change.new_state)?;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderTraceBlock {
    pub pos: BlockPos,
    pub state: BlockStateId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTraceFrame {
    pub timestamp_ms: i32,
    pub changes: Vec<RenderTraceBlock>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderTrace {
    pub simulation_walltime: Duration,
    pub recorded_ticks: u64,
    pub initial_blocks: Vec<RenderTraceBlock>,
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
        let mut frames = Vec::new();
        let mut previous_timestamp = 0;
        loop {
            let timestamp_ms = read_i32(&mut input)?;
            let count = read_u32(&mut input)?;
            if timestamp_ms == END_TIMESTAMP {
                if count != 0 {
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
            frames.push(RenderTraceFrame {
                timestamp_ms,
                changes,
            });
        }
        Ok(Self {
            simulation_walltime,
            recorded_ticks,
            initial_blocks,
            frames,
        })
    }
}

fn read_block(input: &mut impl Read) -> Result<RenderTraceBlock, ReplayError> {
    Ok(RenderTraceBlock {
        pos: BlockPos::new(read_i32(input)?, read_i32(input)?, read_i32(input)?),
        state: BlockStateId(read_u32(input)?),
    })
}

fn read_u16(input: &mut impl Read) -> Result<u16, ReplayError> {
    let mut bytes = [0; 2];
    input.read_exact(&mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
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
    use std::io::Cursor;

    use super::*;

    #[test]
    fn empty_trace_header_round_trips_walltime() {
        let bytes = trace_bytes(FORMAT_VERSION, true);
        let trace = RenderTrace::read(Cursor::new(bytes)).unwrap();

        assert_eq!(trace.simulation_walltime, Duration::from_nanos(123));
        assert_eq!(trace.recorded_ticks, 7);
        assert!(trace.initial_blocks.is_empty());
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
            RenderTrace::read(Cursor::new(trace_bytes(FORMAT_VERSION, false))),
            Err(ReplayError::Io(_))
        ));
    }

    fn trace_bytes(version: u16, complete: bool) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(&PROTOCOL_VERSION.to_le_bytes());
        bytes.extend_from_slice(&123u64.to_le_bytes());
        bytes.extend_from_slice(&7u64.to_le_bytes());
        bytes.extend_from_slice(&0u64.to_le_bytes());
        if complete {
            bytes.extend_from_slice(&END_TIMESTAMP.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        bytes
    }
}
