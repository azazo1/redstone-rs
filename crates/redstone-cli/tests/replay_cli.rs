use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;
use zip::ZipArchive;

const LEVEL_CHUNK_WITH_LIGHT: i32 = 45;
const BLOCK_ENTITY_DATA: i32 = 6;
const BLOCK_EVENT: i32 = 7;
const BLOCK_UPDATE: i32 = 8;
const PLAYER_POSITION: i32 = 72;
const PISTON_BLOCK_ENTITY_TYPE: i32 = 11;

#[test]
fn run_exports_parseable_replay_and_atomically_replaces_target() {
    let directory = TestDirectory::new("replay-run");
    let scenario = write_fixture(directory.path(), false);
    let scenario_text = fs::read_to_string(&scenario).unwrap().replace(
        "strict = true",
        "strict = true\n\n[monitor]\nskip_ticks = 2",
    );
    fs::write(&scenario, scenario_text).unwrap();
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
        updates
            .iter()
            .map(|update| (update.0, update.1))
            .collect::<Vec<_>>(),
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
fn replay_timeline_keeps_recording_time_and_writes_editing_paths() {
    let directory = TestDirectory::new("replay-timeline");
    let structure_path = directory.path().join("machine.nbt");
    let scenario_path = directory.path().join("timeline.toml");
    let replay = directory.path().join("timeline.mcpr");
    fs::write(structure_path, structure()).unwrap();
    fs::write(&scenario_path, timeline_scenario()).unwrap();

    let output = run_command("run", &scenario_path, &replay);
    assert_success(&output);
    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);

    let initial_chunk = packets
        .iter()
        .filter(|packet| packet.id == LEVEL_CHUNK_WITH_LIGHT && packet.timestamp == 0)
        .map(|packet| decode_chunk(packet.payload))
        .find(|(chunk, _)| *chunk == (0, 0))
        .unwrap();
    assert_ne!(initial_chunk.1[section_index(0, 0, 0)], 0);

    let updates = packets
        .iter()
        .filter(|packet| packet.id == BLOCK_UPDATE)
        .map(|packet| decode_block_update(packet.timestamp, packet.payload))
        .filter(|(_, pos, _)| *pos == (0, 0, 0))
        .collect::<Vec<_>>();
    assert_eq!(updates.len(), 4);
    assert_eq!(
        updates.iter().map(|update| update.0).collect::<Vec<_>>(),
        [50, 100, 150, 200]
    );
    assert_eq!(updates[0].2, 0);
    assert_ne!(updates[1].2, 0);
    assert_ne!(updates[2].2, 0);
    assert_ne!(updates[1].2, updates[2].2);
    assert_eq!(updates[3].2, 0);

    let metadata = read_metadata(&replay);
    assert_eq!(metadata["duration"], 200);

    let timelines = read_timelines(&replay);
    let paths = timelines[""].as_array().unwrap();
    assert_eq!(paths.len(), 2);
    assert_eq!(
        paths[0]["keyframes"],
        serde_json::json!([
            {"time": 0, "properties": {"timestamp": 50}},
            {"time": 50, "properties": {"timestamp": 150}}
        ])
    );
    assert_eq!(paths[0]["segments"], serde_json::json!([0]));
    assert_eq!(
        paths[0]["interpolators"],
        serde_json::json!([{"type": "linear", "properties": ["timestamp"]}])
    );

    let pose = packets
        .iter()
        .find(|packet| packet.id == PLAYER_POSITION)
        .map(|packet| decode_player_position(packet.payload))
        .unwrap();
    let position_keyframes = paths[1]["keyframes"].as_array().unwrap();
    assert_eq!(position_keyframes.len(), 2);
    assert_eq!(position_keyframes[0]["time"], 0);
    assert_eq!(position_keyframes[1]["time"], 50);
    for keyframe in position_keyframes {
        assert_eq!(
            keyframe["properties"]["camera:position"],
            serde_json::json!(pose.position)
        );
        let rotation = keyframe["properties"]["camera:rotation"]
            .as_array()
            .unwrap();
        assert!((rotation[0].as_f64().unwrap() - f64::from(pose.yaw)).abs() < 0.0001);
        assert!((rotation[1].as_f64().unwrap() - f64::from(pose.pitch)).abs() < 0.0001);
        assert_eq!(rotation[2], 0.0);
    }
    assert_eq!(paths[1]["segments"], serde_json::json!([0]));
    assert_eq!(
        paths[1]["interpolators"],
        serde_json::json!([{
            "type": {"type": "catmull-rom-spline", "alpha": 0.5},
            "properties": ["camera:rotation", "camera:position"]
        }])
    );
}

#[test]
fn replay_timeline_rejects_invalid_scenario_ranges_and_durations() {
    let directory = TestDirectory::new("replay-timeline-invalid");
    fs::write(directory.path().join("machine.nbt"), structure()).unwrap();
    let invalid_replays = [
        (4, "start_tick = 3\nend_tick = 2"),
        (4, "start_tick = 0\nend_tick = 5"),
        (4, "start_tick = 1\nend_tick = 2\nduration_ms = 0"),
        (4, "start_tick = 2\nend_tick = 2\nduration_ms = 1"),
        (
            42_949_673,
            "start_tick = 0\nend_tick = 42949673\nduration_ms = 1",
        ),
    ];

    for (index, (max_ticks, replay_config)) in invalid_replays.into_iter().enumerate() {
        let scenario = directory.path().join(format!("invalid-{index}.toml"));
        let replay = directory.path().join(format!("invalid-{index}.mcpr"));
        fs::write(
            &scenario,
            format!(
                r#"version = "26.1.2"
mode = "default"
max_ticks = {max_ticks}

[replay]
{replay_config}

[source]
path = "machine.nbt"
initialization = "raw"
"#
            ),
        )
        .unwrap();

        let output = run_command("run", &scenario, &replay);
        assert!(!output.status.success());
        assert!(!replay.exists());
    }
}

#[test]
fn scenario_camera_pose_is_encoded_into_replay() {
    let directory = TestDirectory::new("replay-camera");
    let structure_path = directory.path().join("machine.nbt");
    let scenario_path = directory.path().join("camera.toml");
    let replay = directory.path().join("camera.mcpr");
    fs::write(structure_path, structure()).unwrap();
    fs::write(
        &scenario_path,
        r#"version = "26.1.2"
mode = "default"
max_ticks = 0

[replay.camera]
view_distance = 12
position = [1.25, 20.5, -3.75]
yaw = -45.0
pitch = 30.0

[source]
path = "machine.nbt"
initialization = "raw"
"#,
    )
    .unwrap();

    let output = run_command("run", &scenario_path, &replay);
    assert_success(&output);
    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);
    let packet = packets
        .iter()
        .find(|packet| packet.id == PLAYER_POSITION)
        .unwrap();
    let pose = decode_player_position(packet.payload);

    assert_eq!(pose.position, [1.25, 20.5, -3.75]);
    assert_eq!(pose.yaw, -45.0);
    assert_eq!(pose.pitch, 30.0);
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

#[test]
fn piston_scenario_exports_vanilla_moving_piston_packets() {
    let directory = TestDirectory::new("replay-piston");
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets/scenarios/tripple-piston-extender.toml");
    let replay = directory.path().join("piston.mcpr");

    let output = run_command("run", &scenario, &replay);
    assert_success(&output);
    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);
    assert!(
        packets
            .iter()
            .all(|packet| packet.timestamp == 0 || packet.id != BLOCK_EVENT)
    );
    let mut moving_pistons = 0;
    let mut moved_wool = false;
    for (index, packet) in packets.iter().enumerate() {
        if packet.id != BLOCK_ENTITY_DATA {
            continue;
        }
        let (pos, kind, nbt) = decode_block_entity_data(packet.payload);
        if kind != PISTON_BLOCK_ENTITY_TYPE {
            continue;
        }
        moving_pistons += 1;
        let previous = &packets[index - 1];
        assert_eq!(previous.id, BLOCK_UPDATE);
        assert_eq!(previous.timestamp, packet.timestamp);
        assert_eq!(
            decode_block_update(previous.timestamp, previous.payload).1,
            pos
        );
        assert_eq!(nbt[0], 10);
        for field in [
            b"blockState".as_slice(),
            b"facing".as_slice(),
            b"progress".as_slice(),
            b"extending".as_slice(),
            b"source".as_slice(),
        ] {
            assert!(nbt.windows(field.len()).any(|value| value == field));
        }
        moved_wool |= nbt
            .windows(b"minecraft:orange_wool".len())
            .any(|value| value == b"minecraft:orange_wool");
    }
    assert!(moving_pistons > 0);
    assert!(moved_wool);
}

#[test]
fn piston_animation_exports_vanilla_block_events_for_the_3x3_gate() {
    let directory = TestDirectory::new("replay-piston-animation");
    let scenario =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/piston-gate-3x3.toml");
    let replay = directory.path().join("piston-animation.mcpr");

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario)
        .arg("--replay")
        .arg(&replay)
        .arg("--replay-anim")
        .output()
        .unwrap();
    assert_success(&output);

    let recording = read_recording(&replay);
    let events = parse_packets(&recording)
        .into_iter()
        .filter(|packet| packet.timestamp > 0 && packet.id == BLOCK_EVENT)
        .map(|packet| decode_block_event(packet.timestamp, packet.payload))
        .collect::<Vec<_>>();

    assert!(!events.is_empty());
    assert!(events.iter().any(|event| {
        event.pos == (4, 3, 1) && event.param_a == 0 && event.param_b == 1 && event.block == 128
    }));
    assert!(events.iter().any(|event| {
        event.pos == (8, 7, 1) && event.param_a == 0 && event.param_b == 0 && event.block == 138
    }));
}

#[test]
fn piston_animation_starts_before_slime_and_honey_branch_updates() {
    let directory = TestDirectory::new("replay-piston-sticky-branches");
    let structure_path = directory.path().join("sticky-branches.nbt");
    let scenario_path = directory.path().join("sticky-branches.toml");
    let replay = directory.path().join("sticky-branches.mcpr");
    fs::write(&structure_path, sticky_branch_structure()).unwrap();
    fs::write(&scenario_path, sticky_branch_scenario()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario_path)
        .arg("--replay")
        .arg(&replay)
        .arg("--replay-anim")
        .output()
        .unwrap();
    assert_success(&output);

    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);
    let event_index = packets
        .iter()
        .position(|packet| {
            packet.timestamp > 0
                && packet.id == BLOCK_EVENT
                && decode_block_event(packet.timestamp, packet.payload).pos == (1, 1, 0)
        })
        .unwrap();
    let event = decode_block_event(packets[event_index].timestamp, packets[event_index].payload);
    assert_eq!((event.param_a, event.param_b, event.block), (0, 5, 138));

    let moving_updates = packets
        .iter()
        .enumerate()
        .filter(|(_, packet)| packet.timestamp == packets[event_index].timestamp)
        .filter(|(_, packet)| packet.id == BLOCK_ENTITY_DATA)
        .filter_map(|(index, packet)| {
            let (pos, kind, nbt) = decode_block_entity_data(packet.payload);
            (kind == PISTON_BLOCK_ENTITY_TYPE).then_some((index, pos, nbt))
        })
        .collect::<Vec<_>>();
    assert!(
        moving_updates
            .iter()
            .all(|(index, _, _)| *index > event_index)
    );
    assert!(
        moving_updates
            .iter()
            .any(|(_, pos, nbt)| { *pos == (3, 1, 0) && contains(nbt, b"minecraft:slime_block") })
    );
    assert!(
        moving_updates
            .iter()
            .any(|(_, pos, nbt)| { *pos == (3, 2, 0) && contains(nbt, b"minecraft:stone") })
    );
    assert!(moving_updates.iter().all(|(_, pos, _)| *pos != (3, 0, 0)));
}

#[test]
fn piston_animation_requires_a_replay_output() {
    let directory = TestDirectory::new("replay-piston-animation-argument");
    let scenario = write_fixture(directory.path(), false);

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario)
        .arg("--replay-anim")
        .output()
        .unwrap();

    assert!(!output.status.success());
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

fn sticky_branch_structure() -> Vec<u8> {
    let palette = Value::List(vec![
        block_state_with_properties(
            "minecraft:piston",
            &[("extended", "false"), ("facing", "east")],
        ),
        block_state("minecraft:slime_block"),
        block_state("minecraft:stone"),
        block_state("minecraft:honey_block"),
    ]);
    let blocks = Value::List(vec![
        structure_block_at([1, 1, 0], 0),
        structure_block_at([2, 1, 0], 1),
        structure_block_at([2, 2, 0], 2),
        structure_block_at([2, 0, 0], 3),
    ]);
    fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(4), Value::Int(3), Value::Int(1)]),
        ),
        ("palette".to_owned(), palette),
        ("blocks".to_owned(), blocks),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]))
    .unwrap()
}

fn block_state(name: &str) -> Value {
    Value::Compound(HashMap::from([(
        "Name".to_owned(),
        Value::String(name.to_owned()),
    )]))
}

fn block_state_with_properties(name: &str, properties: &[(&str, &str)]) -> Value {
    Value::Compound(HashMap::from([
        ("Name".to_owned(), Value::String(name.to_owned())),
        (
            "Properties".to_owned(),
            Value::Compound(
                properties
                    .iter()
                    .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
                    .collect(),
            ),
        ),
    ]))
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

fn structure_block_at(pos: [i32; 3], state: i32) -> Value {
    Value::Compound(HashMap::from([
        (
            "pos".to_owned(),
            Value::List(pos.into_iter().map(Value::Int).collect()),
        ),
        ("state".to_owned(), Value::Int(state)),
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

fn timeline_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
max_ticks = 4
strict = true

[replay]
start_tick = 1
end_tick = 3
duration_ms = 50

[source]
path = "machine.nbt"
initialization = "raw"

[[actions]]
tick = 1
type = "break_block"
pos = { x = 0, y = 0, z = 0 }

[[actions]]
tick = 2
type = "set_block"
pos = { x = 0, y = 0, z = 0 }
name = "minecraft:redstone_block"

[[actions]]
tick = 3
type = "set_block"
pos = { x = 0, y = 0, z = 0 }
name = "minecraft:stone"

[[actions]]
tick = 4
type = "break_block"
pos = { x = 0, y = 0, z = 0 }
"#
}

fn sticky_branch_scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 4
strict = true

[source]
path = "sticky-branches.nbt"
initialization = "raw"

[[actions]]
tick = 1
type = "set_block"
pos = { x = 0, y = 1, z = 0 }
name = "minecraft:redstone_block"
"#
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

fn read_metadata(path: &Path) -> serde_json::Value {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut metadata = archive.by_name("metaData.json").unwrap();
    let mut bytes = Vec::new();
    metadata.read_to_end(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn read_timelines(path: &Path) -> serde_json::Value {
    let mut archive = ZipArchive::new(File::open(path).unwrap()).unwrap();
    let mut timelines = archive.by_name("timelines.json").unwrap();
    let mut bytes = Vec::new();
    timelines.read_to_end(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
        Some(
            (0..length)
                .map(|_| cursor.var_int() as u32)
                .collect::<Vec<_>>(),
        )
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
            palette
                .as_ref()
                .map_or(value as u32, |palette| palette[value])
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
    let pos = unpack_block_pos(cursor.u64());
    (timestamp, pos, cursor.var_int() as u32)
}

struct DecodedCameraPose {
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
}

fn decode_player_position(payload: &[u8]) -> DecodedCameraPose {
    let mut cursor = Cursor::new(payload);
    cursor.var_int();
    let position = [cursor.f64(), cursor.f64(), cursor.f64()];
    for _ in 0..3 {
        cursor.f64();
    }
    let yaw = cursor.f32();
    let pitch = cursor.f32();
    DecodedCameraPose {
        position,
        yaw,
        pitch,
    }
}

struct DecodedBlockEvent {
    pos: (i32, i32, i32),
    param_a: u8,
    param_b: u8,
    block: i32,
}

fn decode_block_event(_timestamp: i32, payload: &[u8]) -> DecodedBlockEvent {
    let mut cursor = Cursor::new(payload);
    let pos = unpack_block_pos(cursor.u64());
    let param_a = cursor.u8();
    let param_b = cursor.u8();
    let block = cursor.var_int();
    DecodedBlockEvent {
        pos,
        param_a,
        param_b,
        block,
    }
}

fn decode_block_entity_data(payload: &[u8]) -> ((i32, i32, i32), i32, &[u8]) {
    let mut cursor = Cursor::new(payload);
    let pos = unpack_block_pos(cursor.u64());
    let kind = cursor.var_int();
    (pos, kind, &payload[cursor.offset..])
}

fn unpack_block_pos(packed: u64) -> (i32, i32, i32) {
    let x = sign_extend((packed >> 38) & 0x3ff_ffff, 26) as i32;
    let y = sign_extend(packed & 0xfff, 12) as i32;
    let z = sign_extend((packed >> 12) & 0x3ff_ffff, 26) as i32;
    (x, y, z)
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    ((value << (64 - bits)) as i64) >> (64 - bits)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|value| value == needle)
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

    fn f32(&mut self) -> f32 {
        f32::from_be_bytes(self.bytes(4).try_into().unwrap())
    }

    fn f64(&mut self) -> f64 {
        f64::from_be_bytes(self.bytes(8).try_into().unwrap())
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
