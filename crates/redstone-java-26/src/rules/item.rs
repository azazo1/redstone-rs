include!(concat!(env!("OUT_DIR"), "/item_stack_sizes.rs"));
include!(concat!(env!("OUT_DIR"), "/item_traits.rs"));

#[cfg(test)]
mod tests {
    use super::{
        official_brewing_ingredient, official_compost_chance, official_furnace_fuel,
        official_item_max_stack_size,
    };

    #[test]
    fn official_stack_sizes_cover_comparator_fill_thresholds() {
        assert_eq!(official_item_max_stack_size("minecraft:diamond_sword"), 1);
        assert_eq!(official_item_max_stack_size("minecraft:honey_bottle"), 16);
        assert_eq!(official_item_max_stack_size("minecraft:stone"), 64);
    }

    #[test]
    fn official_item_traits_cover_hopper_container_rules() {
        assert!(official_furnace_fuel("minecraft:coal"));
        assert!(official_furnace_fuel("minecraft:acacia_boat"));
        assert!(official_furnace_fuel("minecraft:acacia_planks"));
        assert!(official_furnace_fuel("minecraft:white_wool"));
        assert!(!official_furnace_fuel("minecraft:warped_planks"));
        assert!(!official_furnace_fuel("minecraft:stone"));
        assert!(official_brewing_ingredient("minecraft:nether_wart"));
        assert!(!official_brewing_ingredient("minecraft:diamond"));
        assert_eq!(official_compost_chance("minecraft:cake"), Some(1.0));
        assert_eq!(official_compost_chance("minecraft:stone"), None);
    }
}
