struct ViewUniform {
    transform: mat4x4<f32>,
    size: vec2<f32>,
    scale: f32,
}

@group(0) @binding(0)
var<uniform> view_uniform: ViewUniform;

struct Vertex {
    @location(0) position: vec2<f32>,
}

struct FxInstance {
    @location(1) position: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) kind: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) kind: f32,
}

@vertex
fn vs_main(vertex: Vertex, fx: FxInstance) -> VertexOutput {
    let pos = fx.position * view_uniform.scale;
    let size = fx.size * view_uniform.scale;

    let transform = mat4x4<f32>(
        vec4<f32>(size.x, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, size.y, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, 1.0, 0.0),
        vec4<f32>(pos, 0.0, 1.0)
    );

    var out: VertexOutput;
    out.position = view_uniform.transform * transform * vec4<f32>(vertex.position, 0.0, 1.0);
    out.uv = vertex.position;
    out.color = fx.color;
    out.kind = fx.kind;
    return out;
}

// Soft glowing orb with a hot white core
fn orb(uv: vec2<f32>, color: vec4<f32>) -> vec4<f32> {
    let r = length(uv - vec2<f32>(0.5)) * 2.0;
    let halo = pow(clamp(1.0 - r, 0.0, 1.0), 2.2);
    let core = pow(clamp(1.0 - r * 2.2, 0.0, 1.0), 3.0);
    let rgb = mix(color.rgb, vec3<f32>(1.0), core * 0.8);
    return vec4<f32>(rgb, (halo + core) * color.a);
}

// Column of light rising from the bottom edge and fading upwards
fn beam(uv: vec2<f32>, color: vec4<f32>) -> vec4<f32> {
    let x = abs(uv.x * 2.0 - 1.0);
    let across = pow(clamp(1.0 - x, 0.0, 1.0), 1.6);
    let along = pow(uv.y, 3.0);
    let core = pow(clamp(1.0 - x * 3.0, 0.0, 1.0), 2.0) * along;
    let rgb = mix(color.rgb, vec3<f32>(1.0), core * 0.5);
    return vec4<f32>(rgb, across * along * color.a);
}

// Thin horizontal line of light, fading out at both ends
fn line(uv: vec2<f32>, color: vec4<f32>) -> vec4<f32> {
    let y = (uv.y - 0.5) * 2.0;
    let glow = exp(-y * y * 12.0);
    let core = exp(-y * y * 120.0);
    let ends = smoothstep(0.0, 0.08, uv.x) * (1.0 - smoothstep(0.92, 1.0, uv.x));
    let rgb = mix(color.rgb, vec3<f32>(1.0), core * 0.7);
    return vec4<f32>(rgb, (glow * 0.6 + core) * ends * color.a);
}

// Hash without sine (Dave Hoskins), stable on every GPU
fn hash(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Puff of smoke: a soft blob broken up by noise, each puff (seed) shaped differently
fn smoke(uv: vec2<f32>, color: vec4<f32>, seed: f32) -> vec4<f32> {
    let r = length(uv - vec2<f32>(0.5)) * 2.0;
    let blob = pow(clamp(1.0 - r, 0.0, 1.0), 1.8);
    let p = uv * 2.2 + vec2<f32>(seed * 37.0, seed * 91.0);
    let n = value_noise(p) * 0.55 + value_noise(p * 2.03 + 5.0) * 0.3
        + value_noise(p * 4.1 + 11.0) * 0.15;
    let wisps = smoothstep(0.25, 0.8, n);
    return vec4<f32>(color.rgb, blob * wisps * color.a);
}

// Four pointed glint
fn glint(uv: vec2<f32>, color: vec4<f32>) -> vec4<f32> {
    let p = abs(uv - vec2<f32>(0.5)) * 2.0;
    let r = length(p);
    let rays = exp(-p.x * 26.0) * (1.0 - p.y) + exp(-p.y * 26.0) * (1.0 - p.x);
    let core = pow(clamp(1.0 - r * 3.0, 0.0, 1.0), 2.0);
    let a = clamp(rays * 0.8 + core, 0.0, 1.0) * clamp(1.0 - r, 0.0, 1.0);
    return vec4<f32>(mix(color.rgb, vec3<f32>(1.0), core), a * color.a);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    if in.kind < 0.5 {
        return orb(in.uv, in.color);
    } else if in.kind < 1.5 {
        return beam(in.uv, in.color);
    } else if in.kind < 2.5 {
        return line(in.uv, in.color);
    } else if in.kind < 3.5 {
        return glint(in.uv, in.color);
    }
    // Smoke: the fraction above 4 is the puff's seed
    return smoke(in.uv, in.color, in.kind - 4.0);
}
