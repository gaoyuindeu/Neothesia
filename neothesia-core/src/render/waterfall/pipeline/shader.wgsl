struct ViewUniform {
    transform: mat4x4<f32>,
    size: vec2<f32>,
    scale: f32,
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
const GLOW_MARGIN: f32 = 10.0;

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
    let d = rounded_box_sdf(in.position.xy, in.note_pos, in.size, in.radius);
    let scale = view_uniform.scale;

    // Outer glow
    if d > 0.5 {
        let glow = exp(-d / (4.0 * scale)) * 0.45;
        return vec4<f32>(in.color, glow);
    }

    let fill_alpha = 1.0 - smoothstep(-0.5, 0.5, d);

    // 0 at the top of the note, 1 at the bottom
    let t = clamp((in.position.y - in.note_pos.y) / max(in.size.y, 1.0), 0.0, 1.0);
    var color = in.color * mix(0.72, 1.12, t);

    // Bright rim just inside the edge
    let rim = 1.0 - smoothstep(0.0, 2.5 * scale, -d);
    color = mix(color, vec3<f32>(1.0), rim * 0.45);

    // Light the note up while it is being played
    let playing = in.note_pos.y <= in.keyboard_y && in.note_pos.y + in.size.y >= in.keyboard_y;
    if playing {
        color = mix(color * 1.25, vec3<f32>(1.0), 0.18);
    }

    let glow = exp(-max(d, 0.0) / (4.0 * scale)) * 0.45;
    return vec4<f32>(color, max(fill_alpha, glow));
}
