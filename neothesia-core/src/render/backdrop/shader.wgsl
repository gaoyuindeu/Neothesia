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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    if u.has_image < 0.5 {
        // Plain black (or the configured color): the notes and light carry the picture
        return vec4<f32>(u.base_color, 1.0);
    }
    var color = sample_cover(uv) * (1.0 - u.dim);

    // Vignette
    let v = (uv - vec2<f32>(0.5)) * vec2<f32>(1.1, 1.3);
    color *= 1.0 - 0.55 * dot(v, v);

    return vec4<f32>(color, 1.0);
}
