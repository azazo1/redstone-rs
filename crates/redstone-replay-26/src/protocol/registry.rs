use std::collections::BTreeMap;

use super::buf::PacketBuf;

const REGISTRY_DATA: &str = include_str!("../../data/registries.txt");
const REQUIRED_TAGS: &str = include_str!("../../data/required-tags.txt");

pub(crate) fn registry_packets() -> Vec<Vec<u8>> {
    REGISTRY_DATA
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (registry, entries) = line
                .split_once('=')
                .expect("embedded registry data must contain equals");
            let entries = entries.split(',').collect::<Vec<_>>();
            let mut output = PacketBuf::new();
            output.write_identifier(&format!("minecraft:{registry}"));
            output.write_len(entries.len());
            for entry in entries {
                output.write_identifier(&format!("minecraft:{entry}"));
                output.write_bool(false);
            }
            output.into_inner()
        })
        .collect()
}

pub(crate) fn required_tags_packet() -> Vec<u8> {
    let mut registries = BTreeMap::<&str, Vec<(&str, &str)>>::new();
    for line in REQUIRED_TAGS.lines().filter(|line| !line.is_empty()) {
        let (key, ids) = line
            .split_once('=')
            .expect("embedded tag data must contain equals");
        let (registry, tag) = key
            .split_once('|')
            .expect("embedded tag data must contain pipe");
        registries.entry(registry).or_default().push((tag, ids));
    }

    let mut output = PacketBuf::new();
    output.write_len(registries.len());
    for (registry, tags) in registries {
        output.write_identifier(&format!("minecraft:{registry}"));
        output.write_len(tags.len());
        for (tag, ids) in tags {
            output.write_identifier(&format!("minecraft:{tag}"));
            if ids.is_empty() {
                output.write_var_i32(0);
                continue;
            }
            let ids = ids.split(',').collect::<Vec<_>>();
            output.write_len(ids.len());
            for id in ids {
                output.write_var_i32(id.parse().expect("embedded tag ID must be an integer"));
            }
        }
    }
    output.into_inner()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registry_order_contains_all_synchronized_registries() {
        assert_eq!(registry_packets().len(), 28);
        let biome = REGISTRY_DATA.lines().next().unwrap();
        let entries = biome.split_once('=').unwrap().1.split(',').collect::<Vec<_>>();
        assert_eq!(entries[40], "plains");
    }

    #[test]
    fn required_tags_bind_enchantment_dialog_and_timeline_registries() {
        let packet = required_tags_packet();
        assert_eq!(packet[0], 8);
        assert!(REQUIRED_TAGS.contains("enchantment|exclusive_set/riptide=19,5"));
        assert!(REQUIRED_TAGS.contains("dialog|quick_actions="));
        assert!(REQUIRED_TAGS.contains("timeline|in_overworld=3,0,2,1"));
        assert!(REQUIRED_TAGS.contains("item|enchantable/head_armor="));
        assert!(REQUIRED_TAGS.contains("entity_type|sensitive_to_smite="));
        assert!(REQUIRED_TAGS.contains("block|soul_speed_blocks=286,287"));
        assert!(REQUIRED_TAGS.contains("damage_type|is_fire="));
        assert!(REQUIRED_TAGS.contains("banner_pattern|pattern_item/flower=12"));
        assert!(REQUIRED_TAGS.contains("entity_type|can_equip_saddle="));
    }
}
