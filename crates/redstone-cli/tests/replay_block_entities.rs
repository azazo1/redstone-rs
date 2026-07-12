use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use fastnbt::Value;
use zip::ZipArchive;

const BLOCK_ENTITY_DATA: i32 = 6;
const LEVEL_CHUNK_WITH_LIGHT: i32 = 45;
const CHEST_BLOCK_ENTITY_TYPE: i32 = 1;
const SIGN_BLOCK_ENTITY_TYPE: i32 = 7;
const HOPPER_BLOCK_ENTITY_TYPE: i32 = 18;

#[test]
fn replay_exports_initial_and_runtime_block_entity_data() {
    let directory = TestDirectory::new();
    let scenario = write_fixture(directory.path());
    let replay = directory.path().join("block-entities.mcpr");

    let output = Command::new(env!("CARGO_BIN_EXE_redstone"))
        .arg("run")
        .arg(&scenario)
        .arg("--replay")
        .arg(&replay)
        .output()
        .unwrap();
    assert_success(&output);

    let recording = read_recording(&replay);
    let packets = parse_packets(&recording);
    let initial_chunks = packets
        .iter()
        .filter(|packet| packet.timestamp == 0 && packet.id == LEVEL_CHUNK_WITH_LIGHT)
        .map(|packet| packet.payload)
        .collect::<Vec<_>>();
    assert!(contains_any(&initial_chunks, b"front_text"));
    assert!(contains_any(&initial_chunks, b"back_text"));
    assert!(contains_any(&initial_chunks, b"initial front"));
    assert!(contains_any(&initial_chunks, b"minecraft:redstone"));
    assert!(contains_any(&initial_chunks, b"minecraft:custom_name"));

    let updates = packets
        .iter()
        .filter(|packet| packet.timestamp == 50 && packet.id == BLOCK_ENTITY_DATA)
        .map(|packet| decode_block_entity_data(packet.payload))
        .collect::<Vec<_>>();
    assert_eq!(
        updates.iter().map(|update| update.kind).collect::<Vec<_>>(),
        [
            SIGN_BLOCK_ENTITY_TYPE,
            HOPPER_BLOCK_ENTITY_TYPE,
            CHEST_BLOCK_ENTITY_TYPE,
        ]
    );

    let sign = &updates[0].nbt;
    assert!(contains(sign, b"front_text"));
    assert!(contains(sign, b"back_text"));
    assert!(contains(sign, b"updated front"));
    assert!(contains(sign, b"updated back"));

    for update in &updates[1..] {
        assert!(contains(update.nbt, b"Items"));
    }
    assert!(contains(updates[1].nbt, b"minecraft:redstone"));
    assert!(contains(updates[2].nbt, b"minecraft:redstone"));
    assert!(contains(updates[1].nbt, b"minecraft:custom_name"));
    assert!(contains(updates[2].nbt, b"minecraft:custom_name"));
    assert_eq!(nbt_int_field(updates[1].nbt, "TransferCooldown"), Some(8));
}

fn write_fixture(directory: &Path) -> PathBuf {
    let structure_path = directory.join("block-entities.nbt");
    let scenario_path = directory.join("block-entities.toml");
    fs::write(&structure_path, structure()).unwrap();
    fs::write(&scenario_path, scenario()).unwrap();
    scenario_path
}

fn structure() -> Vec<u8> {
    let palette = Value::List(vec![
        block_state(
            "minecraft:hopper",
            &[("enabled", "true"), ("facing", "down")],
        ),
        block_state(
            "minecraft:chest",
            &[
                ("facing", "north"),
                ("type", "single"),
                ("waterlogged", "false"),
            ],
        ),
        block_state(
            "minecraft:oak_sign",
            &[("rotation", "0"), ("waterlogged", "false")],
        ),
    ]);
    let blocks = Value::List(vec![
        structure_block(
            [0, 0, 0],
            0,
            HashMap::from([
                (
                    "id".to_owned(),
                    Value::String("minecraft:hopper".to_owned()),
                ),
                ("Items".to_owned(), Value::List(Vec::new())),
                ("TransferCooldown".to_owned(), Value::Int(0)),
            ]),
        ),
        structure_block(
            [0, 1, 0],
            1,
            HashMap::from([
                ("id".to_owned(), Value::String("minecraft:chest".to_owned())),
                (
                    "Items".to_owned(),
                    Value::List(vec![Value::Compound(HashMap::from([
                        ("Slot".to_owned(), Value::Byte(0)),
                        (
                            "id".to_owned(),
                            Value::String("minecraft:redstone".to_owned()),
                        ),
                        ("count".to_owned(), Value::Int(2)),
                        (
                            "components".to_owned(),
                            Value::Compound(HashMap::from([(
                                "minecraft:custom_name".to_owned(),
                                Value::String("{\"text\":\"Signal\"}".to_owned()),
                            )])),
                        ),
                    ]))]),
                ),
            ]),
        ),
        structure_block(
            [3, 0, 0],
            2,
            HashMap::from([
                ("id".to_owned(), Value::String("minecraft:sign".to_owned())),
                ("front_text".to_owned(), sign_text("initial front")),
                ("back_text".to_owned(), sign_text("initial back")),
            ]),
        ),
    ]);
    fastnbt::to_bytes(&HashMap::from([
        ("DataVersion".to_owned(), Value::Int(4790)),
        (
            "size".to_owned(),
            Value::List(vec![Value::Int(4), Value::Int(2), Value::Int(1)]),
        ),
        ("palette".to_owned(), palette),
        ("blocks".to_owned(), blocks),
        ("entities".to_owned(), Value::List(Vec::new())),
    ]))
    .unwrap()
}

fn block_state(name: &str, properties: &[(&str, &str)]) -> Value {
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

fn structure_block(pos: [i32; 3], state: i32, nbt: HashMap<String, Value>) -> Value {
    Value::Compound(HashMap::from([
        (
            "pos".to_owned(),
            Value::List(pos.into_iter().map(Value::Int).collect()),
        ),
        ("state".to_owned(), Value::Int(state)),
        ("nbt".to_owned(), Value::Compound(nbt)),
    ]))
}

fn sign_text(message: &str) -> Value {
    let messages = [message, "", "", ""]
        .into_iter()
        .map(|message| Value::String(format!(r#"{{"text":"{message}"}}"#)))
        .collect::<Vec<_>>();
    Value::Compound(HashMap::from([
        ("messages".to_owned(), Value::List(messages.clone())),
        ("filtered_messages".to_owned(), Value::List(messages)),
        ("color".to_owned(), Value::String("black".to_owned())),
        ("has_glowing_text".to_owned(), Value::Byte(0)),
    ]))
}

fn scenario() -> &'static str {
    r#"version = "26.1.2"
mode = "default"
seed = 0
max_ticks = 1
strict = true

[source]
path = "block-entities.nbt"
initialization = "raw"

[[actions]]
tick = 1
type = "set_block_entity"
pos = { x = 3, y = 0, z = 0 }
data = { kind = "minecraft:sign", fields = { front_text = { messages = ["{\"text\":\"updated front\"}", "", "", ""], filtered_messages = ["{\"text\":\"updated front\"}", "", "", ""], color = "blue", has_glowing_text = true }, back_text = { messages = ["{\"text\":\"updated back\"}", "", "", ""], filtered_messages = ["{\"text\":\"updated back\"}", "", "", ""], color = "red", has_glowing_text = false } } }
"#
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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

struct BlockEntityUpdate<'a> {
    kind: i32,
    nbt: &'a [u8],
}

fn decode_block_entity_data(payload: &[u8]) -> BlockEntityUpdate<'_> {
    let mut cursor = Cursor::new(payload);
    cursor.bytes(8);
    let kind = cursor.var_int();
    BlockEntityUpdate {
        kind,
        nbt: &payload[cursor.offset..],
    }
}

fn contains_any(haystacks: &[&[u8]], needle: &[u8]) -> bool {
    haystacks.iter().any(|haystack| contains(haystack, needle))
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|value| value == needle)
}

fn nbt_int_field(nbt: &[u8], name: &str) -> Option<i32> {
    let name = name.as_bytes();
    let mut pattern = Vec::with_capacity(name.len() + 3);
    pattern.push(3);
    pattern.extend_from_slice(&(name.len() as u16).to_be_bytes());
    pattern.extend_from_slice(name);
    let offset = nbt
        .windows(pattern.len())
        .position(|value| value == pattern)?
        + pattern.len();
    Some(i32::from_be_bytes(
        nbt.get(offset..offset + 4)?.try_into().ok()?,
    ))
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
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "redstone-cli-block-entities-{}-{nonce}",
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
