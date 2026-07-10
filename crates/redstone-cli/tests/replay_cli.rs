use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;
use zip::ZipArchive;

const LEVEL_CHUNK_WITH_LIGHT: i32 = 45;
const BLOCK_UPDATE: i32 = 8;

#[test]
fn run_exports_parseable_replay_and_atomically_replaces_target() {
    let directory = TestDirectory::new("replay-run");
    let scenario = write_fixture(directory.path(), false);
    let replay = directory.path().join("run.mcpr");
    fs::write(&replay, b"old replay").unwrap();

    let output = run_command("run", &scenario, &replay);
    assert_success(&output);
    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);

    let initial = packets
        .iter()
        .filter(|packet| packet.id == LEVEL_CHUNK_WITH_LIGHT && packet.timestamp == 0)
        .map(|packet| decode_chunk(packet.payload))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        initial.keys().copied().collect::<BTreeSet<_>>(),
        chunk_rectangle(-3, 5, -2, 0),
    );
    assert_ne!(initial[&(-2, -1)][section_index(15, 0, 15)], 0);
    assert_ne!(initial[&(1, -1)][section_index(0, 0, 15)], 0);
    assert!(initial[&(4, -1)].iter().all(|state| *state == 0));

    let expanded_at_tick_one = packets
        .iter()
        .filter(|packet| packet.id == LEVEL_CHUNK_WITH_LIGHT && packet.timestamp == 50)
        .map(|packet| decode_chunk(packet.payload).0)
        .collect::<BTreeSet<_>>();
    assert_eq!(expanded_at_tick_one, chunk_rectangle(6, 6, -2, 0));
    let expanded_at_tick_two = packets
        .iter()
        .filter(|packet| packet.id == LEVEL_CHUNK_WITH_LIGHT && packet.timestamp == 100)
        .map(|packet| decode_chunk(packet.payload).0)
        .collect::<BTreeSet<_>>();
    assert_eq!(expanded_at_tick_two, chunk_rectangle(7, 7, -2, 0));

    let updates = packets
        .iter()
        .filter(|packet| packet.id == BLOCK_UPDATE)
        .map(|packet| decode_block_update(packet.timestamp, packet.payload))
        .collect::<Vec<_>>();
    assert_eq!(
        updates.iter().map(|update| (update.0, update.1)).collect::<Vec<_>>(),
        [
            (50, (-17, 0, -1)),
            (50, (16, 0, -1)),
            (50, (80, 0, -1)),
            (100, (-17, 0, -1)),
            (100, (96, 0, -1)),
        ]
    );
}

#[test]
fn single_scenario_test_exports_replay() {
    let directory = TestDirectory::new("replay-test");
    let scenario = write_fixture(directory.path(), false);
    let replay = directory.path().join("test.mcpr");

    let output = run_command("test", &scenario, &replay);
    assert_success(&output);
    assert!(replay.is_file());
    assert!(ZipArchive::new(File::open(replay).unwrap()).is_ok());
}

#[test]
fn assertion_failure_keeps_completed_replay() {
    let directory = TestDirectory::new("replay-failure");
    let scenario = write_fixture(directory.path(), true);
    let replay = directory.path().join("failure.mcpr");

    let output = run_command("test", &scenario, &replay);
    assert!(!output.status.success());
    assert!(replay.is_file());
    assert!(ZipArchive::new(File::open(replay).unwrap()).is_ok());
}

#[test]
fn directory_test_rejects_shared_replay_target() {
    let directory = TestDirectory::new("replay-directory");
    write_fixture(directory.path(), false);
    let replay = directory.path().join("directory.mcpr");

    let output = run_command("test", directory.path(), &replay);
    assert!(!output.status.success());
    assert!(!replay.exists());
}

fn run_command(command: &str, scenario: &Path, replay: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg(command)
        .arg(scenario)
        .arg("--replay")
        .arg(replay)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_fixture(directory: &Path, failing: bool) -> PathBuf {
    let structure_path = directory.join("machine.nbt");
    let scenario_path = directory.join("scenario.toml");
    fs::write(&structure_path, structure()).unwrap();
    fs::write(&scenario_path, scenario(failing)).unwrap();
    scenario_path
}

fn structure() -> Vec<u8> {
    let root = HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(96), Value::Int(1), Value::Int(1)]),
        ),
        (
            "palette".to_owned(),
            Value::List(vec![block_state("minecraft:stone")]),
        ),
        (
            "blocks".to_owned(),
            Value::List(vec![structure_block(0), structure_block(33)]),
        ),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]);
    fastnbt::to_bytes(&root).unwrap()
}

fn block_state(name: &str) -> Value {
    Value::Compound(HashMap::from([(
        "Name".to_owned(),
        Value::String(name.to_owned()),
    )]))
}

fn structure_block(x: i32) -> Value {
    Value::Compound(HashMap::from([
        (
            "pos".to_owned(),
            Value::List(vec![Value::Int(x), Value::Int(0), Value::Int(0)]),
        ),
        ("state".to_owned(), Value::Int(0)),
    ]))
}

fn scenario(failing: bool) -> String {
    let expectation = if failing {
        r#"
[[probes]]
name = "state"
type = "block_state"
pos = { x = -17, y = 0, z = -1 }

[[expectations]]
tick = 2
probe = "state"
equals = 999999
"#
    } else {
        ""
    };
    format!(
        r#"version = "26.1.2"
mode = "default"
seed = 7
max_ticks = 2
strict = true

[source]
path = "machine.nbt"
origin = {{ x = -17, y = 0, z = -1 }}
initialization = "raw"

[[actions]]
tick = 1
type = "set_block"
pos = {{ x = -17, y = 0, z = -1 }}
name = "minecraft:redstone_block"

[[actions]]
tick = 1
type = "set_block"
pos = {{ x = 16, y = 0, z = -1 }}
name = "minecraft:redstone_block"

[[actions]]
tick = 1
type = "set_block"
pos = {{ x = 80, y = 0, z = -1 }}
name = "minecraft:redstone_block"

[[actions]]
tick = 2
type = "set_block"
pos = {{ x = -17, y = 0, z = -1 }}
name = "minecraft:stone"

[[actions]]
tick = 2
type = "set_block"
pos = {{ x = 96, y = 0, z = -1 }}
name = "minecraft:redstone_block"
{expectation}"#
    )
}

fn chunk_rectangle(min_x: i32, max_x: i32, min_z: i32, max_z: i32) -> BTreeSet<(i32, i32)> {
    (min_x..=max_x)
        .flat_map(|x| (min_z..=max_z).map(move |z| (x, z)))
        .collect()
}

fn read_recording(path: &Path) -> Vec<u8> {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut recording = archive.by_name("recording.tmcpr").unwrap();
    let mut bytes = Vec::new();
    recording.read_to_end(&mut bytes).unwrap();
    bytes
}

struct Packet<'a> {
    timestamp: i32,
    id: i32,
    payload: &'a [u8],
}

fn parse_packets(mut bytes: &[u8]) -> Vec<Packet<'_>> {
    let mut packets = Vec::new();
    while !bytes.is_empty() {
        let timestamp = i32::from_be_bytes(bytes[0..4].try_into().unwrap());
        let length = i32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize;
        let packet = &bytes[8..8 + length];
        let mut cursor = Cursor::new(packet);
        let id = cursor.var_int();
        packets.push(Packet {
            timestamp,
            id,
            payload: &packet[cursor.offset..],
        });
        bytes = &bytes[8 + length..];
    }
    packets
}

fn decode_chunk(payload: &[u8]) -> ((i32, i32), Vec<u32>) {
    let mut cursor = Cursor::new(payload);
    let x = cursor.i32();
    let z = cursor.i32();
    let heightmaps = cursor.var_int();
    assert_eq!(heightmaps, 2);
    for _ in 0..heightmaps {
        cursor.var_int();
        let length = cursor.var_int() as usize;
        cursor.bytes(length * 8);
    }
    let section_length = cursor.var_int() as usize;
    let section_bytes = cursor.bytes(section_length);
    let mut sections = Cursor::new(section_bytes);
    let mut blocks = Vec::with_capacity(24 * 4_096);
    for _ in 0..24 {
        sections.u16();
        sections.u16();
        blocks.extend(decode_container(&mut sections, 4_096));
        let _ = decode_container(&mut sections, 64);
    }
    ((x, z), blocks)
}

fn decode_container(cursor: &mut Cursor<'_>, size: usize) -> Vec<u32> {
    let bits = cursor.u8();
    if bits == 0 {
        return vec![cursor.var_int() as u32; size];
    }
    let palette = if bits <= 8 {
        let length = cursor.var_int() as usize;
        Some((0..length).map(|_| cursor.var_int() as u32).collect::<Vec<_>>())
    } else {
        None
    };
    let values_per_long = 64 / bits as usize;
    let storage = (0..size.div_ceil(values_per_long))
        .map(|_| cursor.u64())
        .collect::<Vec<_>>();
    let mask = (1u64 << bits) - 1;
    (0..size)
        .map(|index| {
            let value = (storage[index / values_per_long]
                >> (index % values_per_long * bits as usize)
                & mask) as usize;
            palette.as_ref().map_or(value as u32, |palette| palette[value])
        })
        .collect()
}

fn section_index(x: usize, y: i32, z: usize) -> usize {
    let section = (y.div_euclid(16) + 4) as usize;
    let local_y = y.rem_euclid(16) as usize;
    section * 4_096 + (local_y * 16 + z) * 16 + x
}

fn decode_block_update(timestamp: i32, payload: &[u8]) -> (i32, (i32, i32, i32), u32) {
    let mut cursor = Cursor::new(payload);
    let packed = cursor.u64();
    let x = sign_extend((packed >> 38) & 0x3ff_ffff, 26) as i32;
    let y = sign_extend(packed & 0xfff, 12) as i32;
    let z = sign_extend((packed >> 12) & 0x3ff_ffff, 26) as i32;
    (timestamp, (x, y, z), cursor.var_int() as u32)
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    ((value << (64 - bits)) as i64) >> (64 - bits)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn bytes(&mut self, length: usize) -> &'a [u8] {
        let bytes = &self.bytes[self.offset..self.offset + length];
        self.offset += length;
        bytes
    }

    fn u8(&mut self) -> u8 {
        let value = self.bytes[self.offset];
        self.offset += 1;
        value
    }

    fn u16(&mut self) -> u16 {
        u16::from_be_bytes(self.bytes(2).try_into().unwrap())
    }

    fn i32(&mut self) -> i32 {
        i32::from_be_bytes(self.bytes(4).try_into().unwrap())
    }

    fn u64(&mut self) -> u64 {
        u64::from_be_bytes(self.bytes(8).try_into().unwrap())
    }

    fn var_int(&mut self) -> i32 {
        let mut value = 0u32;
        for index in 0..5 {
            let byte = self.u8();
            value |= u32::from(byte & 0x7f) << (index * 7);
            if byte & 0x80 == 0 {
                return value as i32;
            }
        }
        panic!("invalid var int")
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(name: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-cli-{name}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
