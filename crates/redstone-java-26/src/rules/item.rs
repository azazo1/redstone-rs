include!(concat!(env!("OUT_DIR"), "/item_stack_sizes.rs"));

#[cfg(test)]
mod tests {
    use super::official_item_max_stack_size;

    #[test]
    fn official_stack_sizes_cover_comparator_fill_thresholds() {
        assert_eq!(official_item_max_stack_size("minecraft:diamond_sword"), 1);
        assert_eq!(official_item_max_stack_size("minecraft:honey_bottle"), 16);
        assert_eq!(official_item_max_stack_size("minecraft:stone"), 64);
    }
}
