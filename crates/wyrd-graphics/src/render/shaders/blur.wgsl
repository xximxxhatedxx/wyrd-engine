struct BlurParams {
    direction: vec2<f32>,
    radius: f32,
    sigma: f32,
}

@group(0) @binding(0) var source_texture: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlurParams;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    let position = corners[vertex_index];
    var output: VertexOutput;
    output.position = vec4<f32>(position, 0.0, 1.0);
    output.uv = vec2<f32>(position.x * 0.5 + 0.5, 0.5 - position.y * 0.5);
    return output;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dimensions = vec2<f32>(textureDimensions(source_texture));
    let step_uv = params.direction / dimensions;
    let extent = i32(clamp(ceil(params.radius), 0.0, 32.0));
    let sigma = max(params.sigma, 0.5);
    var weighted_color = vec4<f32>(0.0);
    var total_weight = 0.0;
    for (var offset = -extent; offset <= extent; offset += 1) {
        let distance = f32(offset);
        let weight = exp(-0.5 * distance * distance / (sigma * sigma));
        weighted_color += textureSampleLevel(
            source_texture,
            source_sampler,
            input.uv + step_uv * distance,
            0.0,
        ) * weight;
        total_weight += weight;
    }
    return weighted_color / max(total_weight, 0.000001);
}
