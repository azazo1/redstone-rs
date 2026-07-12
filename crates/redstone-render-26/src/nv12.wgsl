@group(0) @binding(0)
var source: texture_2d<f32>;

@group(0) @binding(1)
var<storage, read_write> output: array<atomic<u32>>;

fn byte_value(value: f32) -> u32 {
    return u32(round(clamp(value, 0.0, 255.0)));
}

fn write_byte(offset: u32, value: u32) {
    let word = offset / 4u;
    let shift = (offset % 4u) * 8u;
    atomicOr(&output[word], value << shift);
}

fn rgb_at(position: vec2<u32>) -> vec3<f32> {
    return textureLoad(source, vec2<i32>(position), 0).rgb;
}

@compute @workgroup_size(8, 8)
fn convert(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(source);
    if id.x >= size.x || id.y >= size.y {
        return;
    }
    let rgb = rgb_at(id.xy);
    let y = 16.0 + 255.0 * dot(rgb, vec3<f32>(0.182586, 0.614231, 0.062007));
    write_byte(id.y * size.x + id.x, byte_value(y));

    if id.x % 2u == 0u && id.y % 2u == 0u {
        let average = (
            rgb
            + rgb_at(id.xy + vec2<u32>(1u, 0u))
            + rgb_at(id.xy + vec2<u32>(0u, 1u))
            + rgb_at(id.xy + vec2<u32>(1u, 1u))
        ) * 0.25;
        let u = 128.0 + 255.0 * dot(average, vec3<f32>(-0.100644, -0.338572, 0.439216));
        let v = 128.0 + 255.0 * dot(average, vec3<f32>(0.439216, -0.398942, -0.040274));
        let uv_offset = size.x * size.y + (id.y / 2u) * size.x + id.x;
        write_byte(uv_offset, byte_value(u));
        write_byte(uv_offset + 1u, byte_value(v));
    }
}
