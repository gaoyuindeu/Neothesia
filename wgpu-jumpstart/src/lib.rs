#![allow(clippy::single_match)]

mod error;
use std::ops::{Deref, DerefMut};

pub use error::GpuInitError;

mod color;
mod gpu;
mod instances;
mod render_pipeline_builder;
mod shape;
mod uniform;

mod transform_uniform;

pub use color::Color;
pub use gpu::{Gpu, Surface};
pub use instances::Instances;
pub use render_pipeline_builder::{
    default_color_target_state, default_fragment, default_render_pipeline, default_vertex,
};
pub use shape::Shape;
pub use transform_uniform::TransformUniform;
pub use uniform::Uniform;
pub use wgpu;

pub struct RenderPass<'a>(wgpu::RenderPass<'a>, wgpu::Extent3d, (u32, u32));

impl<'a> RenderPass<'a> {
    pub fn new(rpass: wgpu::RenderPass<'a>, size: wgpu::Extent3d) -> Self {
        Self(rpass, size, (0, 0))
    }

    /// Size of the current view (the whole target unless [`Self::set_view`] was called)
    pub fn size(&self) -> wgpu::Extent3d {
        self.1
    }

    /// Top left corner of the current view in target pixels
    pub fn origin(&self) -> (u32, u32) {
        self.2
    }

    /// Draw into a part of the target: the viewport maps to this rectangle, `size()`
    /// reports it and scissor rects set through [`Self::set_view_scissor`] are relative to it
    pub fn set_view(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let width = width.max(1);
        let height = height.max(1);
        self.0
            .set_viewport(x as f32, y as f32, width as f32, height as f32, 0.0, 1.0);
        self.0.set_scissor_rect(x, y, width, height);
        self.1 = wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        self.2 = (x, y);
    }

    /// Scissor rect in view coordinates, clipped to the view
    pub fn set_view_scissor(&mut self, x: u32, y: u32, width: u32, height: u32) {
        let x = x.min(self.1.width);
        let y = y.min(self.1.height);
        let width = width.min(self.1.width - x);
        let height = height.min(self.1.height - y);
        self.0
            .set_scissor_rect(self.2.0 + x, self.2.1 + y, width, height);
    }

    /// Scissor to the whole view
    pub fn reset_view_scissor(&mut self) {
        let size = self.1;
        self.set_view_scissor(0, 0, size.width, size.height);
    }
}

impl<'a> Deref for RenderPass<'a> {
    type Target = wgpu::RenderPass<'a>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a> DerefMut for RenderPass<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
