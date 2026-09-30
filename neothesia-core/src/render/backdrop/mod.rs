//! Full screen background: a user image (darkened, optionally blurred) or a gradient.

use std::{path::Path, time::Duration};

use bytemuck::{Pod, Zeroable};
use wgpu_jumpstart::{Gpu, Shape, Uniform, wgpu};

use super::image::texture::Texture;

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct BackdropUniform {
    screen: [f32; 2],
    image: [f32; 2],
    base_color: [f32; 3],
    time: f32,
    has_image: f32,
    dim: f32,
    blur: f32,
    _pad: f32,
}

pub struct Backdrop {
    render_pipeline: wgpu::RenderPipeline,
    fullscreen_quad: Shape,
    uniform: Uniform<BackdropUniform>,
    texture_bind_group: wgpu::BindGroup,
    queue: wgpu::Queue,
}

/// Largest image side kept in memory, bigger images are downscaled
const MAX_IMAGE_SIDE: u32 = 3840;

fn load_image(path: &Path) -> Option<image::RgbaImage> {
    let img = match image::open(path) {
        Ok(img) => img,
        Err(err) => {
            log::warn!("Could not load background image {}: {err}", path.display());
            return None;
        }
    };

    let img = if img.width().max(img.height()) > MAX_IMAGE_SIDE {
        img.resize(
            MAX_IMAGE_SIDE,
            MAX_IMAGE_SIDE,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };

    Some(img.to_rgba8())
}

impl Backdrop {
    /// `image`: optional background image, `dim`: 0 = as is, 1 = black,
    /// `blur`: blur radius in screen pixels
    pub fn new(
        gpu: &Gpu,
        image: Option<&Path>,
        base_color: (u8, u8, u8),
        dim: f32,
        blur: f32,
    ) -> Self {
        let device = &gpu.device;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Backdrop::shader"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(include_str!(
                "./shader.wgsl"
            ))),
        });

        let loaded = image.and_then(load_image);
        let (texture, image_size) = match &loaded {
            Some(img) => (
                Texture::from_image(
                    device,
                    &gpu.queue,
                    (img.as_raw(), img.width(), img.height()),
                    Some("Backdrop image"),
                ),
                [img.width() as f32, img.height() as f32],
            ),
            None => (
                Texture::from_image(device, &gpu.queue, (&[0, 0, 0, 255], 1, 1), None),
                [1.0, 1.0],
            ),
        };

        let base_color = wgpu_jumpstart::Color::from(base_color).into_linear_rgb();

        let uniform = Uniform::new(
            device,
            BackdropUniform {
                screen: [1.0, 1.0],
                image: image_size,
                base_color,
                time: 0.0,
                has_image: if loaded.is_some() { 1.0 } else { 0.0 },
                dim: dim.clamp(0.0, 1.0),
                blur: blur.max(0.0),
                _pad: 0.0,
            },
            wgpu::ShaderStages::FRAGMENT,
        );

        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Backdrop texture layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let texture_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Backdrop texture"),
            layout: &texture_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&texture.sampler),
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&uniform.bind_group_layout), Some(&texture_layout)],
            immediate_size: 0,
        });

        let target = wgpu::ColorTargetState {
            format: gpu.texture_format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        };

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            layout: Some(&layout),
            fragment: Some(wgpu_jumpstart::default_fragment(&shader, &[Some(target)])),
            ..wgpu_jumpstart::default_render_pipeline(wgpu_jumpstart::default_vertex(
                &shader,
                &[Some(Shape::layout())],
            ))
        });

        Self {
            render_pipeline,
            fullscreen_quad: Shape::new_fullscreen_quad(device),
            uniform,
            texture_bind_group,
            queue: gpu.queue.clone(),
        }
    }

    /// `screen`: physical size of the window
    pub fn update(&mut self, delta: Duration, screen: (f32, f32)) {
        self.uniform.data.time += delta.as_secs_f32();
        self.uniform.data.screen = [screen.0.max(1.0), screen.1.max(1.0)];
        self.uniform.update(&self.queue);
    }

    pub fn render<'a>(&'a self, render_pass: &mut wgpu::RenderPass<'a>) {
        render_pass.set_pipeline(&self.render_pipeline);
        render_pass.set_bind_group(0, &self.uniform.bind_group, &[]);
        render_pass.set_bind_group(1, &self.texture_bind_group, &[]);
        render_pass.set_vertex_buffer(0, self.fullscreen_quad.vertex_buffer.slice(..));
        render_pass.set_index_buffer(
            self.fullscreen_quad.index_buffer.slice(..),
            wgpu::IndexFormat::Uint16,
        );
        render_pass.draw_indexed(0..self.fullscreen_quad.indices_len, 0, 0..1);
    }
}
