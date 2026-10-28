struct Instance {
    bounds: vec4<f32>,
    color: vec4<f32>,
    params: vec4<f32>,
    extra: vec4<f32>,
    clip_info: vec4<u32>,
}

struct ClipRegion {
    bounds: vec4<f32>,
    corner_radius: vec4<f32>, // .x = corner_radius, .yzw = padding for alignment with mat4x4<f32>
    inverse_transform: mat4x4<f32>,
}

struct Viewport {
    size: vec2<f32>,
}

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(0) @binding(1) var<storage, read> instances: array<Instance>;
@group(0) @binding(2) var<storage, read> transforms: array<mat4x4<f32>>;
@group(0) @binding(3) var<storage, read> gradient_stops: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read> path_segments: array<vec4<f32>>;
@group(0) @binding(5) var<storage, read> clip_regions: array<ClipRegion>;
@group(1) @binding(0) var scene_texture: texture_2d<f32>;
@group(1) @binding(1) var scene_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) instance_index: u32,
    @location(2) uv: vec2<f32>,
    @location(3) screen_position: vec2<f32>,
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOutput {
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let bounds = instances[instance_index].bounds;
    let pixel = bounds.xy + corner * bounds.zw;
    let transformed = transforms[instance_index] * vec4<f32>(pixel, 0.0, 1.0);
    let screen = transformed.xy / max(abs(transformed.w), 0.000001);

    var output: VertexOutput;
    output.position = vec4<f32>(
        screen.x / viewport.size.x * 2.0 - 1.0,
        1.0 - screen.y / viewport.size.y * 2.0,
        0.0,
        1.0,
    );
    output.local = corner * bounds.zw;
    output.instance_index = instance_index;
    output.uv = mix(instances[instance_index].extra.xy, instances[instance_index].extra.zw, corner);
    output.screen_position = screen;
    return output;
}

fn rounded_rect_distance(position: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let half_size = size * 0.5;
    let r = clamp(radius, 0.0, min(half_size.x, half_size.y));
    let q = abs(position - half_size) - (half_size - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn line_segment_distance(position: vec2<f32>, segment: vec4<f32>) -> f32 {
    let start = segment.xy;
    let delta = segment.zw - start;
    let length_squared = dot(delta, delta);
    let projection = select(
        0.0,
        clamp(dot(position - start, delta) / length_squared, 0.0, 1.0),
        length_squared > 0.000001,
    );
    return distance(position, start + projection * delta);
}

fn gradient_color(instance: Instance, position: vec2<f32>, size: vec2<f32>) -> vec4<f32> {
    let kind = u32(instance.params.y);
    var amount = 0.0;
    if kind == 4u {
        let direction = vec2<f32>(sin(instance.extra.z), -cos(instance.extra.z));
        let extent = (size.x * abs(direction.x) + size.y * abs(direction.y)) * 0.5;
        amount = dot(position - size * 0.5, direction) / max(2.0 * extent, 0.000001) + 0.5;
    } else {
        let center = instance.params.zw;
        amount = distance(position, center) / max(instance.extra.z, 0.000001);
    }
    let first = u32(instance.extra.x);
    let count = u32(instance.extra.y);
    if count == 0u {
        return instance.color;
    }
    let first_position = gradient_stops[first * 2u].x;
    if amount <= first_position {
        let color = gradient_stops[first * 2u + 1u];
        return vec4<f32>(color.rgb, color.a * instance.color.a);
    }
    var previous_position = first_position;
    var previous_color = gradient_stops[first * 2u + 1u];
    for (var index = 1u; index < count; index += 1u) {
        let current_position = gradient_stops[(first + index) * 2u].x;
        let current_color = gradient_stops[(first + index) * 2u + 1u];
        if amount <= current_position {
            let factor = clamp(
                (amount - previous_position) / max(current_position - previous_position, 0.000001),
                0.0,
                1.0,
            );
            let color = mix(previous_color, current_color, factor);
            return vec4<f32>(color.rgb, color.a * instance.color.a);
        }
        previous_position = current_position;
        previous_color = current_color;
    }
    return vec4<f32>(previous_color.rgb, previous_color.a * instance.color.a);
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let instance = instances[input.instance_index];
    for (var index = 0u; index < instance.clip_info.y; index += 1u) {
        let clip = clip_regions[instance.clip_info.x + index];
        let clip_position = clip.inverse_transform * vec4<f32>(input.screen_position, 0.0, 1.0);
        let local = clip_position.xy / max(abs(clip_position.w), 0.000001);
        if local.x < clip.bounds.x
            || local.y < clip.bounds.y
            || local.x >= clip.bounds.x + clip.bounds.z
            || local.y >= clip.bounds.y + clip.bounds.w
        {
            discard;
        }
        if clip.corner_radius.x > 0.0 {
            let clip_local = local - clip.bounds.xy;
            let clip_size = clip.bounds.zw;
            if rounded_rect_distance(clip_local, clip_size, clip.corner_radius.x) > 0.0 {
                discard;
            }
        }
    }
    let size = instance.bounds.zw;
    let radius = instance.params.x;
    let kind = u32(instance.params.y);
    let distance = rounded_rect_distance(input.local, size, radius);
    var color = instance.color;

    if kind == 1u {
        let inner_size = max(size - vec2<f32>(instance.params.z * 2.0), vec2<f32>(0.0));
        let inner_radius = max(radius - instance.params.z, 0.0);
        let inner_position = input.local - vec2<f32>(instance.params.z);
        if rounded_rect_distance(inner_position, inner_size, inner_radius) <= 0.0 {
            discard;
        }
        let outline_style = u32(instance.extra.x);
        if outline_style != 0u {
            let angle = atan2(input.local.y - size.y * 0.5, input.local.x - size.x * 0.5);
            let perimeter = 2.0 * (size.x + size.y);
            let phase = fract((angle + 3.14159265359) / 6.28318530718) * perimeter;
            let period = select(12.0, 6.0, outline_style == 2u);
            let visible_length = select(8.0, 2.0, outline_style == 2u);
            if phase - floor(phase / period) * period > visible_length {
                discard;
            }
        }
    } else if kind == 2u {
        let padding = instance.params.z;
        let shadow_size = max(size - vec2<f32>(padding * 2.0), vec2<f32>(0.0));
        let shadow_position = input.local - vec2<f32>(padding);
        let shadow_distance = rounded_rect_distance(
            shadow_position,
            shadow_size,
            instance.params.x,
        );
        let original_position = input.local - instance.extra.xy;
        if rounded_rect_distance(
            original_position,
            shadow_size,
            instance.params.x,
        ) <= 0.0 {
            discard;
        }
        let sigma = max(instance.params.w, 0.5);
        let edge_gaussian =
            exp(-0.5 * shadow_distance * shadow_distance / (sigma * sigma));
        let coverage = select(
            1.0 - 0.5 * edge_gaussian,
            0.5 * edge_gaussian,
            shadow_distance > 0.0,
        );
        color.a *= coverage;
    } else if kind == 3u {
        let center = size * 0.5 + instance.extra.zw;
        let position = input.local - center;
        let distance_to_ring = abs(length(position) - radius);
        let half_stroke = instance.params.z * 0.5;
        let angle = atan2(position.y, position.x);
        var normalized = fract((angle - instance.extra.x) / 6.28318530718);
        if instance.extra.y > 0.5 {
            normalized = fract((instance.extra.x - angle) / 6.28318530718);
        }
        if distance_to_ring > half_stroke || normalized > instance.params.w {
            discard;
        }
    } else if kind == 4u || kind == 5u {
        color = gradient_color(instance, input.local, size);
    } else if kind == 6u {
        if distance > 0.0 {
            discard;
        }
        color = textureSample(scene_texture, scene_sampler, input.uv) * instance.color;
    } else if kind == 10u {
        if distance > 0.0 {
            discard;
        }
        let sigma = max(instance.params.z * 0.5, 0.5);
        let depth = max(-distance, 0.0);
        color.a *= exp(-0.5 * depth * depth / (sigma * sigma));
    } else if kind == 11u {
        color = vec4<f32>(color.rgb * color.a, color.a);
    } else if kind == 8u {
        var path_distance = 1000000.0;
        let start = u32(instance.extra.x);
        let count = u32(instance.extra.y);
        for (var index = 0u; index < count; index += 1u) {
            path_distance = min(
                path_distance,
                line_segment_distance(input.local, path_segments[start + index]),
            );
        }
        let half_width = max(instance.params.z, 1.0) * 0.5;
        let coverage = 1.0 - smoothstep(half_width - 0.75, half_width + 0.75, path_distance);
        if coverage <= 0.0 {
            discard;
        }
        color.a *= coverage;
    } else if distance > 0.0 {
        discard;
    }
    return color;
}
