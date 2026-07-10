use redstone_core::{BlockPos, SparseWorld};
use serde_json::Value;

use crate::StateDefinition;

pub(super) fn decorated_pot_output(world: &SparseWorld, pos: BlockPos) -> i32 {
    let Some(data) = world.block_entity(pos) else {
        return 0;
    };
    data.fields
        .get("item")
        .or_else(|| data.fields.get("Item"))
        .map_or(0, item_stack_signal)
}

pub(super) fn lectern_output(world: &SparseWorld, pos: BlockPos, state: &StateDefinition) -> i32 {
    if !state.bool_property("has_book") {
        return 0;
    }
    let Some(data) = world.block_entity(pos) else {
        return 0;
    };
    let page_count = field_i64(&data.fields, &["page_count", "PageCount"])
        .or_else(|| {
            data.fields
                .get("Book")
                .or_else(|| data.fields.get("book"))
                .and_then(book_page_count)
        })
        .unwrap_or(0)
        .max(0);
    if page_count <= 1 {
        return 15;
    }
    let page = field_i64(&data.fields, &["Page", "page"])
        .unwrap_or(0)
        .clamp(0, page_count - 1);
    (page * 14 / (page_count - 1) + 1).clamp(1, 15) as i32
}

pub(super) fn jukebox_output(world: &SparseWorld, pos: BlockPos) -> i32 {
    let Some(data) = world.block_entity(pos) else {
        return 0;
    };
    let stack = data
        .fields
        .get("RecordItem")
        .or_else(|| data.fields.get("record_item"));
    let item_id = stack
        .and_then(item_id)
        .or_else(|| data.fields.get("item_id").and_then(Value::as_str));
    let song = stack
        .and_then(jukebox_song_component)
        .or_else(|| item_id.and_then(|item| item.strip_prefix("minecraft:music_disc_")));
    song.map_or(0, jukebox_song_output)
}

fn field_i64(fields: &std::collections::BTreeMap<String, Value>, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| fields.get(*key).and_then(Value::as_i64))
}

fn book_page_count(stack: &Value) -> Option<i64> {
    let object = stack.as_object()?;
    let components = object
        .get("components")
        .or_else(|| object.get("Components"))?
        .as_object()?;
    let content = components
        .get("minecraft:written_book_content")
        .or_else(|| components.get("minecraft:writable_book_content"))?
        .as_object()?;
    i64::try_from(content.get("pages")?.as_array()?.len()).ok()
}

fn item_stack_signal(stack: &Value) -> i32 {
    let Some(object) = stack.as_object() else {
        return 0;
    };
    let count = object
        .get("count")
        .or_else(|| object.get("Count"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    if count <= 0 {
        return 0;
    }
    let max_stack_size = object
        .get("components")
        .or_else(|| object.get("Components"))
        .and_then(Value::as_object)
        .and_then(|components| components.get("minecraft:max_stack_size"))
        .and_then(Value::as_i64)
        .or_else(|| item_id(stack).map(super::super::item::official_item_max_stack_size))
        .unwrap_or(64)
        .clamp(1, 99);
    (1 + count.clamp(1, max_stack_size) * 14 / max_stack_size).clamp(1, 15) as i32
}

fn item_id(stack: &Value) -> Option<&str> {
    stack.as_object()?.get("id")?.as_str()
}

fn jukebox_song_component(stack: &Value) -> Option<&str> {
    let components = stack.as_object()?.get("components")?.as_object()?;
    let song = components.get("minecraft:jukebox_playable")?;
    song.as_str()
        .or_else(|| song.as_object()?.get("song")?.as_str())
        .map(|song| song.strip_prefix("minecraft:").unwrap_or(song))
}

fn jukebox_song_output(song: &str) -> i32 {
    match song {
        "13" => 1,
        "cat" => 2,
        "blocks" => 3,
        "chirp" => 4,
        "far" => 5,
        "mall" => 6,
        "mellohi" => 7,
        "stal" => 8,
        "strad" | "lava_chicken" => 9,
        "ward" | "tears" => 10,
        "11" | "creator_music_box" => 11,
        "wait" | "creator" => 12,
        "pigstep" | "precipice" => 13,
        "otherside" | "relic" => 14,
        "5" => 15,
        _ => 0,
    }
}
