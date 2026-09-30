struct BackdropUniform {
    screen: vec2<f32>,
    image: vec2<f32>,
    base_color: vec3<f32>,
    time: f32,
    has_image: f32,
    dim: f32,
    blur: f32,
    _pad: f32,
}

@group(0) @binding(0)
var<uniform> u: BackdropUniform;

@group(1) @binding(0)
var t_image: texture_2d<f32>;
@group(1) @binding(1)
var s_image: sampler;

struct Vertex {
    @location(0) position: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // 0,0 = top left
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(vertex.position, 0.0, 1.0);
    out.uv = vec2<f32>(vertex.position.x + 1.0, 1.0 - vertex.position.y) / 2.0;
    return out;
}

fn sample_cover(uv: vec2<f32>) -> vec3<f32> {
    // Scale the image to cover the whole screen, cropping the overflow
    let cover = max(u.screen.x / u.image.x, u.screen.y / u.image.y);
    let scaled = u.image * cover;
    let offset = (u.screen - scaled) / 2.0;
    let tuv = (uv * u.screen - offset) / scaled;

    var color = textureSampleLevel(t_image, s_image, tuv, 0.0).rgb;
    if u.blur <= 0.0 {
        return color;
    }

    // Two rings of taps, a cheap soft blur
    let radius = u.blur / scaled;
    var sum = color;
    var weight = 1.0;
    for (var i = 0; i < 12; i++) {
        let a = f32(i) * 0.5235988;
        let dir = vec2<f32>(cos(a), sin(a));
        sum += textureSampleLevel(t_image, s_image, tuv + dir * radius, 0.0).rgb * 0.8;
        sum += textureSampleLevel(t_image, s_image, tuv + dir * radius * 0.5, 0.0).rgb;
        weight += 1.8;
    }
    return sum / weight;
}

fn haze(uv: vec2<f32>, center: vec2<f32>, size: f32) -> f32 {
    let d = (uv - center) * vec2<f32>(u.screen.x / u.screen.y, 1.0);
    return exp(-dot(d, d) / (size * size));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    var color: vec3<f32>;

    if u.has_image > 0.5 {
        color = sample_cover(uv) * (1.0 - u.dim);
    } else {
        // Deep gradient with two slowly drifting glows
        let top = u.base_color + vec3<f32>(0.020, 0.018, 0.050);
        let bottom = u.base_color + vec3<f32>(0.002, 0.002, 0.008);
        color = mix(top, bottom, uv.y);

        let t = u.time * 0.05;
        let a = haze(uv, vec2<f32>(0.3 + 0.15 * sin(t), 0.3 + 0.08 * cos(t * 1.3)), 0.45);
        let b = haze(uv, vec2<f32>(0.72 + 0.12 * cos(t * 0.8), 0.45 + 0.1 * sin(t * 1.1)), 0.4);
        color += vec3<f32>(0.035, 0.015, 0.075) * a + vec3<f32>(0.0, 0.03, 0.06) * b;
    }

    // Vignette
    let v = (uv - vec2<f32>(0.5)) * vec2<f32>(1.1, 1.3);
    color *= 1.0 - 0.55 * dot(v, v);

    return vec4<f32>(color, 1.0);
}
