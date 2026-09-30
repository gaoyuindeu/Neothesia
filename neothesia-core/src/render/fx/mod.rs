//! Additive light effects drawn over the keyboard: sparks rising from pressed keys,
//! light columns, key bloom and the glowing hit line.

use std::time::Duration;

use bytemuck::{Pod, Zeroable};
use wgpu_jumpstart::{Color, Gpu, Instances, Shape, TransformUniform, Uniform, wgpu};

#[repr(C)]
#[derive(Debug, Copy, Clone, Pod, Zeroable)]
struct FxInstance {
    position: [f32; 2],
    size: [f32; 2],
    color: [f32; 4],
    kind: f32,
}

const KIND_ORB: f32 = 0.0;
const KIND_BEAM: f32 = 1.0;
const KIND_LINE: f32 = 2.0;

impl FxInstance {
    fn attributes() -> [wgpu::VertexAttribute; 4] {
        wgpu::vertex_attr_array!(1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32)
    }

    fn layout(attributes: &[wgpu::VertexAttribute]) -> wgpu::VertexBufferLayout<'_> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FxInstance>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes,
        }
    }
}

struct FxPipeline {
    render_pipeline: wgpu::RenderPipeline,
    quad: Shape,
    instances: Instances<FxInstance>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    transform_uniform_bind_group: wgpu::BindGroup,
}

impl FxPipeline {
    fn new(gpu: &Gpu, transform_uniform: &Uniform<TransformUniform>) -> Self {
        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("FxPipeline::shader"),
                source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                    "./shader.wgsl"
                ))),
            });

        let layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&transform_uniform.bind_group_layout)],
                immediate_size: 0,
            });

        // Additive blending: overlapping light adds up instead of covering
        let target = wgpu::ColorTargetState {
            format: gpu.texture_format,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            write_mask: wgpu::ColorWrites::ALL,
        };

        let attrs = FxInstance::attributes();
        let render_pipeline = gpu
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                layout: Some(&layout),
                fragment: Some(wgpu_jumpstart::default_fragment(&shader, &[Some(target)])),
                ..wgpu_jumpstart::default_render_pipeline(wgpu_jumpstart::default_vertex(
                    &shader,
                    &[Some(Shape::layout()), Some(FxInstance::layout(&attrs))],
                ))
            });

        Self {
            render_pipeline,
            quad: Shape::new_quad(&gpu.device),
            instances: Instances::new(&gpu.device, 20_000),
            device: gpu.device.clone(),
            queue: gpu.queue.clone(),
            transform_uniform_bind_group: transform_uniform.bind_group.clone(),
        }
    }

    fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        if self.instances.is_empty() {
            return;
        }
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.transform_uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.quad.vertex_buffer.slice(..));
        render_pass.set_vertex_buffer(1, self.instances.buffer.slice(..));
        render_pass.set_index_buffer(self.quad.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        render_pass.draw_indexed(0..self.quad.indices_len, 0, 0..self.instances.len());
    }
}

struct Particle {
    pos: [f32; 2],
    vel: [f32; 2],
    age: f32,
    life: f32,
    size: f32,
    color: [f32; 3],
    /// Phase of the sideways wobble
    phase: f32,
}

/// Small xorshift generator, good enough for visual noise
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next()
    }
}

#[derive(Clone, Copy, Default)]
struct KeyFx {
    /// Seconds since the key went down, None when released
    held_for: Option<f32>,
    /// Light intensity: 1 while held, fading out after release
    intensity: f32,
    /// Extra brightness right after the key goes down, decays quickly
    flash: f32,
    color: [f32; 3],
    x: f32,
    width: f32,
    /// Fractional particles carried over between frames
    emit_carry: f32,
}

/// How long the light lingers after a key is released
const RELEASE_TIME: f32 = 0.35;

/// A key that is currently pressed, as seen by the effects
pub struct FxKey {
    pub id: usize,
    pub x: f32,
    pub width: f32,
    pub color: Color,
}

pub struct FxRenderer {
    pipeline: FxPipeline,
    particles: Vec<Particle>,
    keys: Vec<KeyFx>,
    rng: Rng,
    time: f32,
}

const MAX_PARTICLES: usize = 6000;

impl FxRenderer {
    pub fn new(gpu: &Gpu, transform: &Uniform<TransformUniform>, keys_count: usize) -> Self {
        Self {
            pipeline: FxPipeline::new(gpu, transform),
            particles: Vec::with_capacity(MAX_PARTICLES),
            keys: vec![KeyFx::default(); keys_count],
            rng: Rng(0x9E37_79B9),
            time: 0.0,
        }
    }

    /// Advance the simulation.
    ///
    /// `pressed`: keys pressed this frame, `line_y`: top edge of the keyboard,
    /// `line_x`/`line_w`: horizontal extent of the keyboard
    pub fn update(
        &mut self,
        delta: Duration,
        pressed: &[FxKey],
        line_x: f32,
        line_w: f32,
        line_y: f32,
    ) {
        let dt = delta.as_secs_f32().min(0.1);
        self.time += dt;

        self.update_keys(dt, pressed, line_y);
        self.update_particles(dt);
        self.build_instances(line_x, line_w, line_y);
    }

    fn update_keys(&mut self, dt: f32, pressed: &[FxKey], line_y: f32) {
        let mut is_pressed = vec![false; self.keys.len()];

        for key in pressed {
            let Some(state) = self.keys.get_mut(key.id) else {
                continue;
            };
            is_pressed[key.id] = true;

            let color = key.color.into_linear_rgb();
            // Lift the color towards white a bit, light looks better slightly desaturated
            let color = color.map(|c| c * 0.7 + 0.3);
            state.color = color;
            state.x = key.x;
            state.width = key.width;
            state.intensity = 1.0;

            let x = key.x + key.width / 2.0;

            let just_pressed = state.held_for.is_none();
            if just_pressed {
                state.flash = 1.0;
            }
            let held = state.held_for.get_or_insert(0.0);
            *held += dt;

            // Burst of sparks on note on, then a steady trickle while held
            let emit = if just_pressed {
                36.0
            } else {
                state.emit_carry + dt * 60.0
            };
            let count = emit.floor();
            state.emit_carry = emit - count;

            for _ in 0..count as usize {
                if self.particles.len() >= MAX_PARTICLES {
                    break;
                }
                let burst = just_pressed;
                let speed = if burst {
                    self.rng.range(180.0, 520.0)
                } else {
                    self.rng.range(90.0, 260.0)
                };
                let spread = if burst { 0.55 } else { 0.25 };
                let angle = self.rng.range(-spread, spread);

                self.particles.push(Particle {
                    pos: [x + self.rng.range(-0.4, 0.4) * key.width, line_y],
                    vel: [angle.sin() * speed, -angle.cos() * speed],
                    age: 0.0,
                    life: self.rng.range(0.6, 1.6),
                    size: self.rng.range(5.0, 14.0),
                    // Slight random tint and sparkle, so the sparks don't look flat
                    color: {
                        let white = self.rng.range(0.0, 0.5);
                        let tint = [
                            self.rng.range(0.85, 1.15),
                            self.rng.range(0.85, 1.15),
                            self.rng.range(0.85, 1.15),
                        ];
                        [0, 1, 2].map(|i| (color[i] * tint[i]) * (1.0 - white) + white)
                    },
                    phase: self.rng.range(0.0, std::f32::consts::TAU),
                });
            }
        }

        for (state, pressed) in self.keys.iter_mut().zip(is_pressed) {
            if !pressed {
                state.held_for = None;
                state.emit_carry = 0.0;
                state.intensity = (state.intensity - dt / RELEASE_TIME).max(0.0);
            }
            state.flash *= (-dt * 7.0).exp();
        }
    }

    fn update_particles(&mut self, dt: f32) {
        let time = self.time;
        self.particles.retain_mut(|p| {
            p.age += dt;
            if p.age >= p.life {
                return false;
            }

            // Buoyancy, air drag and a gentle sideways wobble
            p.vel[1] -= 60.0 * dt;
            let drag = (1.0 - 1.6 * dt).max(0.0);
            p.vel[0] *= drag;
            p.vel[1] *= drag;
            p.vel[0] += (time * 3.0 + p.phase).sin() * 40.0 * dt;

            p.pos[0] += p.vel[0] * dt;
            p.pos[1] += p.vel[1] * dt;
            true
        });
    }

    fn build_instances(&mut self, line_x: f32, line_w: f32, line_y: f32) {
        let instances = &mut self.pipeline.instances.data;
        instances.clear();

        // Hit line along the top of the keyboard
        instances.push(FxInstance {
            position: [line_x, line_y - 4.0 - 15.0],
            size: [line_w, 30.0],
            color: [0.55, 0.75, 1.0, 0.5],
            kind: KIND_LINE,
        });

        // Light columns and bloom, for held keys and the ones fading out
        for state in self.keys.iter().filter(|k| k.intensity > 0.01) {
            let [r, g, b] = state.color;
            let cx = state.x + state.width / 2.0;
            // Ease out, so the light doesn't cut off abruptly
            let intensity = state.intensity * state.intensity;
            let boost = 1.0 + 1.5 * state.flash;

            let beam_w = state.width * 1.6;
            let beam_h = 260.0 * (0.7 + 0.3 * intensity);
            instances.push(FxInstance {
                position: [cx - beam_w / 2.0, line_y - beam_h],
                size: [beam_w, beam_h],
                color: [r, g, b, 0.55 * intensity * boost],
                kind: KIND_BEAM,
            });

            let bloom = state.width * 5.0 * (0.9 + 0.2 * boost);
            instances.push(FxInstance {
                position: [cx - bloom / 2.0, line_y - bloom / 2.0],
                size: [bloom, bloom],
                color: [r, g, b, 0.45 * intensity * boost],
                kind: KIND_ORB,
            });

            instances.push(FxInstance {
                position: [cx - state.width * 1.5, line_y - 4.0 - 10.0],
                size: [state.width * 3.0, 20.0],
                color: [r, g, b, 0.9 * intensity],
                kind: KIND_LINE,
            });
        }

        for particle in &self.particles {
            let t = particle.age / particle.life;
            let fade = (1.0 - t) * (1.0 - t);
            let size = particle.size * (1.0 - 0.6 * t);
            let [r, g, b] = particle.color;
            instances.push(FxInstance {
                position: [particle.pos[0] - size / 2.0, particle.pos[1] - size / 2.0],
                size: [size, size],
                color: [r, g, b, fade],
                kind: KIND_ORB,
            });
        }
    }

    /// Drop all particles, e.g. after seeking
    pub fn reset(&mut self) {
        self.particles.clear();
        for key in self.keys.iter_mut() {
            *key = KeyFx::default();
        }
    }

    pub fn prepare(&mut self) {
        self.pipeline
            .instances
            .update(&self.pipeline.device, &self.pipeline.queue);
    }

    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        self.pipeline.render(render_pass);
    }
}
