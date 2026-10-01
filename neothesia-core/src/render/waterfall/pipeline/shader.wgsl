struct ViewUniform {
    transform: mat4x4<f32>,
    size: vec2<f32>,
    scale: f32,
    // Top left of the view in target pixels, @builtin(position) is relative to the target
    origin: vec2<f32>,
}

struct TimeUniform {
    time: f32,
    speed: f32,
}

@group(0) @binding(0)
var<uniform> view_uniform: ViewUniform;

@group(1) @binding(0)
var<uniform> time_uniform: TimeUniform;

struct Vertex {
    @location(0) position: vec2<f32>,
}

struct NoteInstance {
    @location(1) n_position: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) radius: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,

    @location(0) size: vec2<f32>,
    @location(1) color: vec3<f32>,
    @location(2) radius: f32,
    @location(3) note_pos: vec2<f32>,
    @location(4) keyboard_y: f32,
}

// Extra space around each note for its outer glow, in physical pixels
const GLOW_MARGIN: f32 = 8.0;

@vertex
fn vs_main(vertex: Vertex, note: NoteInstance) -> VertexOutput {
    let speed = time_uniform.speed;

    let size = vec2<f32>(note.size.x * view_uniform.scale, note.size.y * abs(speed));

    // In an ideal world this should not be hard-coded
    let keyboard_h = view_uniform.size.y / 5.0;
    let keyboard_y = view_uniform.size.y - keyboard_h;

    var pos = vec2<f32>(note.n_position.x * view_uniform.scale, keyboard_y);

    if speed > 0.0 {
        // If notes are falling from top to down, we need to adjust the position,
        // as their start is on bottom of the quad rather than top
        pos.y -= size.y;
    }

    // Offset position by playback time
    pos.y -= (note.n_position.y - time_uniform.time) * speed;

    // Draw a slightly bigger quad, so that the glow around the note fits in
    let margin = GLOW_MARGIN * view_uniform.scale;
    let quad_pos = pos - vec2<f32>(margin);
    let quad_size = size + vec2<f32>(margin * 2.0);

    let transform = mat4x4<f32>(
        vec4<f32>(quad_size.x, 0.0,    0.0, 0.0),
        vec4<f32>(0.0,    quad_size.y, 0.0, 0.0),
        vec4<f32>(0.0,    0.0,    1.0, 0.0),
        vec4<f32>(quad_pos.x,  quad_pos.y,  0.0, 1.0)
    );

    var out: VertexOutput;
    out.position = view_uniform.transform * transform * vec4<f32>(vertex.position, 0.0, 1.0);
    out.note_pos = pos;
    out.keyboard_y = keyboard_y;

    out.size = size;
    out.color = note.color;
    out.radius = note.radius * view_uniform.scale;

    return out;
}

// Signed distance to a rounded rectangle, negative inside
fn rounded_box_sdf(frag_coord: vec2<f32>, position: vec2<f32>, size: vec2<f32>, radius: f32) -> f32 {
    let half = size / 2.0;
    let r = min(radius, min(half.x, half.y));
    let to_center = frag_coord - position - half;
    let q = abs(to_center) - half + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let frag = in.position.xy - view_uniform.origin;
    let d = rounded_box_sdf(frag, in.note_pos, in.size, in.radius);
    let scale = view_uniform.scale;

    // Light the note up while it is being played
    let playing = in.note_pos.y <= in.keyboard_y && in.note_pos.y + in.size.y >= in.keyboard_y;
    let lit = select(1.0, 1.6, playing);

    // Outer glow, thin: a glass bar gives off a soft halo rather than a blob
    if d > 0.5 {
        let glow = exp(-d / (3.0 * scale)) * 0.35 * lit;
        return vec4<f32>(in.color, glow);
    }

    let fill_alpha = 1.0 - smoothstep(-0.5, 0.5, d);
    let local = frag - in.note_pos;
    let uv = clamp(local / max(in.size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));

    // Two tones along the bar: deep at the trailing (top) end, luminous and paler at the
    // leading end that reaches the keys first
    let deep = in.color * vec3<f32>(0.45, 0.6, 0.9);
    let bright = mix(in.color, vec3<f32>(0.85, 1.0, 1.0), 0.15) * 1.15;
    var color = mix(deep, bright, smoothstep(0.0, 1.0, uv.y));

    // Glass body: clear in the middle, denser and brighter towards the edges
    let half_w = max(min(in.size.x, in.size.y) * 0.5, 1.0);
    let inner = clamp(-d / half_w, 0.0, 1.0);
    color *= mix(1.25, 0.7, pow(inner, 0.6));

    // Soft sheen down the left side of the bar
    let sx = (uv.x - 0.25) * in.size.x / (2.0 * scale);
    color += vec3<f32>(0.4) * exp(-sx * sx) * (0.3 + 0.7 * uv.y);

    // Bright rim just inside the edge
    let rim = 1.0 - smoothstep(0.0, 1.6 * scale, -d);
    color = mix(color, mix(in.color, vec3<f32>(1.0), 0.65), rim * 0.8);

    if playing {
        color = mix(color * 1.3, vec3<f32>(1.0), 0.2);
    }

    // Translucent: the dark backdrop shows through the middle of the bar
    let body_alpha = mix(0.95, 0.75, inner);
    let glow = exp(-max(d, 0.0) / (3.0 * scale)) * 0.35;
    return vec4<f32>(color, max(fill_alpha * body_alpha, glow));
}
