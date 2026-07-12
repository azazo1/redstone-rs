use redstone_java_26::{BlockBehavior, StateDefinition};

use super::mesh::Color;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MaterialClass {
    Functional,
    Dye,
    Wood,
    Stone,
    Glass,
    Earth,
    Sand,
    Metal,
    Organic,
    Nether,
    End,
    Prismarine,
    Sculk,
    Unknown,
}

pub(crate) fn block_color(state: &StateDefinition) -> Color {
    let active = state.power > 0
        || state.powered
        || state.lit
        || state.bool_property("triggered")
        || state.bool_property("crafting");
    let emission = if active { 0.72 } else { 0.0 };
    let path = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
    let (_, rgb, perturb) = base_color(path, state);
    let rgb = if perturb {
        perturb_color(rgb, normalized_material(path))
    } else {
        rgb
    };
    [rgb[0], rgb[1], rgb[2], emission]
}

fn base_color(
    path: &str,
    state: &StateDefinition,
) -> (MaterialClass, [f32; 3], bool) {
    let functional = match state.behavior {
        BlockBehavior::Wire => Some(if state.power > 0 {
            [0.95, 0.06, 0.03]
        } else {
            [0.32, 0.03, 0.02]
        }),
        BlockBehavior::RedstoneBlock => Some([0.72, 0.03, 0.03]),
        BlockBehavior::Torch { .. } => Some([0.96, 0.28, 0.08]),
        BlockBehavior::Repeater | BlockBehavior::Comparator => Some([0.83, 0.78, 0.68]),
        BlockBehavior::Lamp | BlockBehavior::CopperBulb => Some(if state.lit {
            [1.0, 0.68, 0.16]
        } else {
            [0.35, 0.24, 0.12]
        }),
        BlockBehavior::Piston { sticky: true } => Some([0.34, 0.58, 0.24]),
        BlockBehavior::Piston { sticky: false } => Some([0.62, 0.48, 0.27]),
        BlockBehavior::Lever | BlockBehavior::Button { .. } => Some([0.58, 0.52, 0.43]),
        _ => None,
    };
    if let Some(rgb) = functional {
        return (MaterialClass::Functional, rgb, false);
    }
    if let Some(rgb) = dye_color(path) {
        return (MaterialClass::Dye, rgb, false);
    }
    if let Some(rgb) = wood_color(path) {
        return (MaterialClass::Wood, rgb, true);
    }
    if path.contains("slime") {
        return (MaterialClass::Organic, [0.43, 0.72, 0.24], false);
    }
    if path.contains("honey") {
        return (MaterialClass::Organic, [0.88, 0.58, 0.12], false);
    }
    if path.contains("glass") || path.contains("ice") {
        return (MaterialClass::Glass, [0.46, 0.72, 0.78], true);
    }
    if path.contains("iron") {
        return (MaterialClass::Metal, [0.72, 0.74, 0.73], true);
    }
    if path.contains("gold") {
        return (MaterialClass::Metal, [0.91, 0.7, 0.13], true);
    }
    if path.contains("copper") {
        return (MaterialClass::Metal, [0.68, 0.42, 0.25], true);
    }
    if path.contains("obsidian") {
        return (MaterialClass::Stone, [0.12, 0.08, 0.18], true);
    }
    if path.contains("prismarine") || path.contains("sea_lantern") {
        return (MaterialClass::Prismarine, [0.31, 0.63, 0.58], true);
    }
    if path.contains("sculk") {
        return (MaterialClass::Sculk, [0.04, 0.2, 0.22], true);
    }
    if path.contains("end_stone") || path.contains("purpur") {
        return (MaterialClass::End, [0.78, 0.78, 0.48], true);
    }
    if path.contains("netherrack")
        || path.contains("nether_brick")
        || path.contains("crimson")
        || path.contains("warped")
        || path.contains("soul_")
    {
        return (MaterialClass::Nether, [0.42, 0.16, 0.18], true);
    }
    if path.contains("sand") {
        return (MaterialClass::Sand, [0.78, 0.7, 0.48], true);
    }
    if path.contains("dirt")
        || path.contains("mud")
        || path.contains("grass")
        || path.contains("mycelium")
        || path.contains("podzol")
    {
        return (MaterialClass::Earth, [0.42, 0.3, 0.17], true);
    }
    if let Some(rgb) = stone_color(path) {
        return (MaterialClass::Stone, rgb, true);
    }
    let palette = [
        [0.48, 0.55, 0.58],
        [0.56, 0.48, 0.52],
        [0.48, 0.56, 0.49],
        [0.57, 0.54, 0.43],
        [0.45, 0.51, 0.6],
        [0.58, 0.48, 0.42],
        [0.48, 0.57, 0.56],
        [0.55, 0.5, 0.61],
    ];
    let index = fnv1a(normalized_material(path).as_bytes()) as usize % palette.len();
    (MaterialClass::Unknown, palette[index], true)
}

fn dye_color(path: &str) -> Option<[f32; 3]> {
    if !["wool", "concrete", "terracotta", "stained_glass"]
        .iter()
        .any(|suffix| path.contains(suffix))
    {
        return None;
    }
    [
        ("light_blue", [0.31, 0.59, 0.78]),
        ("light_gray", [0.62, 0.62, 0.58]),
        ("magenta", [0.7, 0.3, 0.73]),
        ("orange", [0.9, 0.46, 0.13]),
        ("yellow", [0.9, 0.78, 0.16]),
        ("lime", [0.5, 0.7, 0.16]),
        ("pink", [0.86, 0.49, 0.61]),
        ("gray", [0.29, 0.31, 0.31]),
        ("cyan", [0.15, 0.5, 0.57]),
        ("purple", [0.5, 0.25, 0.66]),
        ("blue", [0.24, 0.3, 0.7]),
        ("brown", [0.45, 0.29, 0.16]),
        ("green", [0.33, 0.43, 0.14]),
        ("red", [0.65, 0.2, 0.16]),
        ("black", [0.11, 0.12, 0.14]),
        ("white", [0.86, 0.87, 0.82]),
    ]
    .into_iter()
    .find_map(|(name, rgb)| path.starts_with(name).then_some(rgb))
}

fn wood_color(path: &str) -> Option<[f32; 3]> {
    [
        ("dark_oak", [0.26, 0.17, 0.08]),
        ("pale_oak", [0.78, 0.73, 0.62]),
        ("mangrove", [0.46, 0.2, 0.18]),
        ("cherry", [0.78, 0.55, 0.53]),
        ("spruce", [0.38, 0.26, 0.12]),
        ("birch", [0.78, 0.7, 0.45]),
        ("jungle", [0.62, 0.4, 0.2]),
        ("acacia", [0.66, 0.32, 0.18]),
        ("bamboo", [0.67, 0.64, 0.25]),
        ("crimson", [0.46, 0.19, 0.28]),
        ("warped", [0.14, 0.48, 0.45]),
        ("oak", [0.58, 0.42, 0.22]),
    ]
    .into_iter()
    .find_map(|(name, rgb)| path.contains(name).then_some(rgb))
}

fn stone_color(path: &str) -> Option<[f32; 3]> {
    [
        ("deepslate", [0.27, 0.28, 0.3]),
        ("blackstone", [0.2, 0.17, 0.2]),
        ("cobblestone", [0.43, 0.43, 0.42]),
        ("granite", [0.58, 0.39, 0.32]),
        ("diorite", [0.72, 0.72, 0.69]),
        ("andesite", [0.48, 0.5, 0.5]),
        ("basalt", [0.31, 0.3, 0.31]),
        ("quartz", [0.84, 0.82, 0.76]),
        ("tuff", [0.38, 0.42, 0.38]),
        ("stone", [0.46, 0.47, 0.48]),
        ("brick", [0.55, 0.3, 0.24]),
    ]
    .into_iter()
    .find_map(|(name, rgb)| path.contains(name).then_some(rgb))
}

fn normalized_material(path: &str) -> &str {
    [
        "_pressure_plate",
        "_stained_glass_pane",
        "_fence_gate",
        "_trapdoor",
        "_stairs",
        "_slab",
        "_wall",
        "_fence",
        "_door",
        "_button",
    ]
    .into_iter()
    .find_map(|suffix| path.strip_suffix(suffix))
    .unwrap_or(path)
}

fn perturb_color(rgb: [f32; 3], key: &str) -> [f32; 3] {
    let hash = fnv1a(key.as_bytes());
    let (mut hue, mut saturation, mut lightness) = rgb_to_hsl(rgb);
    hue = (hue + (((hash & 0xff) as f32 / 255.0) * 8.0 - 4.0) / 360.0).rem_euclid(1.0);
    saturation = (saturation + (((hash >> 8 & 0xff) as f32 / 255.0) * 0.08 - 0.04))
        .clamp(0.0, 1.0);
    lightness = (lightness + (((hash >> 16 & 0xff) as f32 / 255.0) * 0.12 - 0.06))
        .clamp(0.04, 0.96);
    hsl_to_rgb(hue, saturation, lightness)
}

fn fnv1a(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c9dc5, |hash, byte| {
        (hash ^ u32::from(*byte)).wrapping_mul(0x01000193)
    })
}

fn rgb_to_hsl(rgb: [f32; 3]) -> (f32, f32, f32) {
    let max = rgb.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let min = rgb.into_iter().fold(f32::INFINITY, f32::min);
    let lightness = (max + min) * 0.5;
    if (max - min).abs() <= f32::EPSILON {
        return (0.0, 0.0, lightness);
    }
    let delta = max - min;
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue = if max == rgb[0] {
        ((rgb[1] - rgb[2]) / delta).rem_euclid(6.0)
    } else if max == rgb[1] {
        (rgb[2] - rgb[0]) / delta + 2.0
    } else {
        (rgb[0] - rgb[1]) / delta + 4.0
    } / 6.0;
    (hue, saturation, lightness)
}

fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let sector = hue * 6.0;
    let x = chroma * (1.0 - (sector.rem_euclid(2.0) - 1.0).abs());
    let rgb = match sector.floor() as i32 {
        0 => [chroma, x, 0.0],
        1 => [x, chroma, 0.0],
        2 => [0.0, chroma, x],
        3 => [0.0, x, chroma],
        4 => [x, 0.0, chroma],
        _ => [chroma, 0.0, x],
    };
    let offset = lightness - chroma * 0.5;
    [rgb[0] + offset, rgb[1] + offset, rgb[2] + offset]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use redstone_java_26::{Java26Registry, StateResolver};

    use super::*;

    #[test]
    fn dye_and_material_families_are_distinct_and_stable() {
        let mut registry = Java26Registry::new();
        let red = color(&mut registry, "minecraft:red_wool");
        let blue = color(&mut registry, "minecraft:blue_wool");
        let stone = color(&mut registry, "minecraft:stone");
        let granite = color(&mut registry, "minecraft:granite");

        assert_ne!(red[..3], blue[..3]);
        assert_ne!(stone[..3], granite[..3]);
        assert_eq!(stone, color(&mut registry, "minecraft:stone"));
    }

    #[test]
    fn shape_suffixes_share_a_material_key() {
        assert_eq!(normalized_material("oak_stairs"), "oak");
        assert_eq!(normalized_material("oak_slab"), "oak");
        assert_eq!(normalized_material("deepslate_tile_wall"), "deepslate_tile");
    }

    #[test]
    fn emission_is_independent_from_static_material_color() {
        let mut registry = Java26Registry::new();
        let off = state(&mut registry, "minecraft:redstone_lamp", [("lit", "false")]);
        let on = state(&mut registry, "minecraft:redstone_lamp", [("lit", "true")]);
        assert_eq!(block_color(&off)[3], 0.0);
        assert!(block_color(&on)[3] > 0.0);
    }

    fn color(registry: &mut Java26Registry, name: &str) -> Color {
        block_color(&state(registry, name, []))
    }

    fn state<const N: usize>(
        registry: &mut Java26Registry,
        name: &str,
        overrides: [(&str, &str); N],
    ) -> StateDefinition {
        let overrides = overrides
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect::<BTreeMap<_, _>>();
        let properties = registry
            .complete_state_properties(name, &overrides)
            .unwrap();
        let state = registry.resolve_state(name, &properties).unwrap();
        registry.resolve_state_id(state).unwrap().clone()
    }
}
