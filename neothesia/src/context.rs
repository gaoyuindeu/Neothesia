use std::sync::Arc;

use crate::{
    NeothesiaEvent, TransformUniform, config::Config, input_manager::InputManager,
    output_manager::OutputManager, utils::window::WindowState,
};
use neothesia_core::render::{QuadRendererFactory, TextRendererFactory};
use wgpu_jumpstart::{Gpu, Uniform};
use winit::event_loop::EventLoopProxy;

use winit::window::Window;

pub struct Context {
    pub window: Arc<Window>,

    /// The main view: the window without the side bar. Scenes lay out and read the
    /// cursor in this space.
    pub window_state: WindowState,
    /// The whole window
    pub full_window_state: WindowState,
    /// Logical width taken by the side bar on the left of the main view
    view_left: f32,
    pub gpu: Gpu,

    pub transform: Uniform<TransformUniform>,
    pub text_renderer_factory: TextRendererFactory,
    pub quad_renderer_factory: QuadRendererFactory,

    pub output_manager: OutputManager,
    pub input_manager: InputManager,
    pub config: Config,

    pub proxy: EventLoopProxy<NeothesiaEvent>,

    /// Last frame timestamp
    pub frame_timestamp: std::time::Instant,

    #[cfg(debug_assertions)]
    pub fps_ticker: neothesia_core::utils::fps_ticker::Fps,
}

impl Drop for Context {
    fn drop(&mut self) {
        self.config.save();
    }
}

impl Context {
    pub fn new(
        window: Arc<Window>,
        window_state: WindowState,
        proxy: EventLoopProxy<NeothesiaEvent>,
        gpu: Gpu,
    ) -> Self {
        let transform_uniform = Uniform::new(
            &gpu.device,
            TransformUniform::default(),
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
        );

        let config = Config::new();

        let text_renderer_factory = TextRendererFactory::new(&gpu);
        let quad_renderer_factory = QuadRendererFactory::new(&gpu, &transform_uniform);

        Self {
            window,

            full_window_state: window_state.clone(),
            window_state,
            view_left: 0.0,
            gpu,
            transform: transform_uniform,
            text_renderer_factory,
            quad_renderer_factory,

            output_manager: Default::default(),
            input_manager: InputManager::new(proxy.clone()),
            config,
            proxy,
            frame_timestamp: std::time::Instant::now(),

            #[cfg(debug_assertions)]
            fps_ticker: Default::default(),
        }
    }

    /// Track a window event in both window states
    pub fn window_event(&mut self, event: &winit::event::WindowEvent) {
        self.full_window_state.window_event(event);
        self.sync_view();
    }

    pub fn view_left(&self) -> f32 {
        self.view_left
    }

    /// Left edge of the main view in physical pixels
    pub fn view_left_px(&self) -> u32 {
        let full = self.full_window_state.physical_size.width;
        ((self.view_left as f64 * self.full_window_state.scale_factor).round() as u32)
            .min(full.saturating_sub(1))
    }

    /// Move the left edge of the main view; scenes see a smaller window
    pub fn set_view_left(&mut self, x: f32) {
        if self.view_left != x {
            self.view_left = x;
            self.sync_view();
            self.resize();
        }
    }

    fn sync_view(&mut self) {
        let left = self.view_left_px();
        let full = &self.full_window_state;
        let mut view = full.clone();
        view.physical_size.width = full.physical_size.width.saturating_sub(left).max(1);
        view.logical_size = view.physical_size.to_logical(full.scale_factor);
        view.cursor_physical_position.x -= left as f64;
        view.cursor_logical_position = view.cursor_physical_position.to_logical(full.scale_factor);
        self.window_state = view;
    }

    pub fn resize(&mut self) {
        self.transform.data.update(
            self.window_state.physical_size.width as f32,
            self.window_state.physical_size.height as f32,
            self.window_state.scale_factor as f32,
        );
        self.transform
            .data
            .set_origin(self.view_left_px() as f32, 0.0);
        self.transform.update(&self.gpu.queue);
    }
}
