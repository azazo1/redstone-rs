struct Camera {
    view_projection: mat4x4<f32>,
};

struct Light {
    view_projection: mat4x4<f32>,
};

@group(0) @binding(0)
var<uniform> camera: Camera;

@group(0) @binding(1)
var<uniform> light: Light;

@group(0) @binding(2)
var shadow_texture: texture_depth_2d;

@group(0) @binding(3)
var shadow_sampler: sampler_comparison;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) instance_position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) world_position: vec3<f32>,
    @location(3) shadow_position: vec4<f32>,
};

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let position = input.position + input.instance_position;
    output.clip_position = camera.view_projection * vec4<f32>(position, 1.0);
    output.normal = input.normal.xyz;
    output.color = input.color;
    output.world_position = position;
    output.shadow_position = light.view_projection * vec4<f32>(position, 1.0);
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let light_direction = normalize(vec3<f32>(0.42, 0.82, 0.38));
    let diffuse = max(dot(normalize(input.normal), light_direction), 0.0);
    let height_fill = clamp(input.world_position.y * 0.002 + 0.18, 0.08, 0.32);
    let shadow_ndc = input.shadow_position.xyz / input.shadow_position.w;
    let shadow_uv = vec2<f32>(shadow_ndc.x * 0.5 + 0.5, shadow_ndc.y * -0.5 + 0.5);
    var visibility = 1.0;
    if all(shadow_uv >= vec2<f32>(0.0)) && all(shadow_uv <= vec2<f32>(1.0)) {
        visibility = textureSampleCompare(
            shadow_texture,
            shadow_sampler,
            shadow_uv,
            shadow_ndc.z - 0.0015,
        );
    }
    let lighting = 0.28 + height_fill + diffuse * (0.18 + visibility * 0.4);
    let emission = input.color.a;
    let rgb = input.color.rgb * lighting + input.color.rgb * emission;
    let encoded = select(
        1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055,
        12.92 * rgb,
        rgb <= vec3<f32>(0.0031308),
    );
    return vec4<f32>(encoded, 1.0);
}
