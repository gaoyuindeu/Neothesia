#![allow(clippy::collapsible_match, clippy::single_match)]

mod context;
mod icons;
mod input_manager;
mod output_manager;
mod scene;
mod song;
mod utils;

use std::{sync::Arc, time::Duration};

use context::Context;
use scene::workspace::Workspace;
use utils::window::WindowState;

use midi_file::midly::MidiMessage;
use neothesia_core::{config, render};
use wgpu_jumpstart::{Gpu, Surface, TransformUniform};
use winit::{
    application::ApplicationHandler,
    event::{ElementState, MouseButton, TouchPhase, WindowEvent},
    event_loop::{EventLoop, EventLoopProxy},
    keyboard::NamedKey,
};

use crate::utils::window::WinitEvent;

#[derive(Debug)]
pub enum NeothesiaEvent {
    /// Go to playing scene
    Play(song::Song),
    FreePlay(Option<song::Song>),
    /// Leave the current scene (back / Esc / song finished)
    MainMenu(Option<song::Song>),
    /// A menu page opened in the workspace was closed, with the song as edited there
    CloseMenu(Option<song::Song>),
    MidiInput {
        /// The MIDI channel that this message is associated with.
        channel: u8,
        /// The MIDI message type and associated data.
        message: MidiMessage,
    },
    Exit,
}

struct Neothesia {
    context: Context,
    workspace: Workspace,
    // We are dropping surface last, because of some wgpu internal ref-counting errors that cause libwayland crasch
    surface: Surface,
    is_occluded: bool,
    frame_dump: Option<FrameDump>,
}

/// Debugging aid: `NEOTHESIA_FRAME_DUMP=<dir>` saves the frames listed in
/// `NEOTHESIA_FRAME_DUMP_AT` (comma separated frame numbers, default 90) as PNG files and
/// quits after the last one
struct FrameDump {
    dir: std::path::PathBuf,
    at: Vec<u64>,
    frame: u64,
}

impl FrameDump {
    fn from_env() -> Option<Self> {
        let dir = std::path::PathBuf::from(std::env::var_os("NEOTHESIA_FRAME_DUMP")?);
        let mut at: Vec<u64> = std::env::var("NEOTHESIA_FRAME_DUMP_AT")
            .unwrap_or_else(|_| "90".into())
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        at.sort_unstable();
        std::fs::create_dir_all(&dir).ok()?;
        Some(Self { dir, at, frame: 0 })
    }
}

impl Neothesia {
    fn new(mut context: Context, surface: Surface) -> Self {
        context.resize();
        let workspace = Workspace::new(&mut context);
        context.gpu.submit();

        Self {
            context,
            surface,
            workspace,
            is_occluded: false,
            frame_dump: FrameDump::from_env(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: &WindowEvent,
    ) {
        self.context.window_event(event);

        match event {
            // Windows sets size to 0 on minimise
            WindowEvent::Resized(ps) if ps.width > 0 && ps.height > 0 => {
                self.surface.resize_swap_chain(
                    self.context.full_window_state.physical_size.width,
                    self.context.full_window_state.physical_size.height,
                );

                self.context.resize();
                self.context.window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                self.context.resize();
            }
            WindowEvent::KeyboardInput { .. } => {
                use winit::keyboard::Key;

                let modifiers = self.context.window_state.modifiers_state;

                if event.key_released(Key::Named(NamedKey::F11))
                    || (event.key_pressed(Key::Character("f")) && modifiers.control_key())
                {
                    if self.context.window.fullscreen().is_some() {
                        self.context.window.set_fullscreen(None);
                    } else {
                        let monitor = self.context.window.current_monitor();
                        if let Some(monitor) = monitor {
                            let f = winit::window::Fullscreen::Borderless(Some(monitor));
                            self.context.window.set_fullscreen(Some(f));
                        } else {
                            let f = winit::window::Fullscreen::Borderless(None);
                            self.context.window.set_fullscreen(Some(f));
                        }
                    }
                    return;
                }
            }
            WindowEvent::RedrawRequested => {
                if self.is_occluded {
                    return;
                }

                let delta = self.context.frame_timestamp.elapsed();
                self.context.frame_timestamp = std::time::Instant::now();

                self.update(delta);
                self.render();
                profiling::finish_frame!();
            }
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Occluded(is_occluded) => {
                self.is_occluded = *is_occluded;
            }
            _ => {}
        }

        if !event.redraw_requested() {
            self.workspace.window_event(&mut self.context, event);
        }
    }

    fn user_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        event: NeothesiaEvent,
    ) {
        match event {
            NeothesiaEvent::Play(song) => {
                self.workspace.play(&mut self.context, song);
            }
            NeothesiaEvent::FreePlay(song) => {
                self.workspace.freeplay(&mut self.context, song);
            }
            NeothesiaEvent::MainMenu(song) => {
                self.workspace.back(&mut self.context, song);
            }
            NeothesiaEvent::CloseMenu(song) => {
                self.workspace.menu_closed(&mut self.context, song);
            }
            NeothesiaEvent::MidiInput { channel, message } => {
                self.workspace
                    .midi_event(&mut self.context, channel, &message);
            }
            NeothesiaEvent::Exit => {
                event_loop.exit();
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {
        self.context.window.request_redraw();
    }

    #[profiling::function]
    fn update(&mut self, delta: Duration) {
        #[cfg(debug_assertions)]
        self.context.fps_ticker.tick();

        self.workspace.update(&mut self.context, delta);
    }

    #[profiling::function]
    fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture) => texture,
            wgpu::CurrentSurfaceTexture::Occluded | wgpu::CurrentSurfaceTexture::Timeout => {
                return;
            }
            wgpu::CurrentSurfaceTexture::Suboptimal(texture) => {
                drop(texture);
                self.surface.configure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                let size = self.context.window.inner_size();
                self.surface = self
                    .context
                    .gpu
                    .recreate_surface(self.context.window.clone().into(), size.width, size.height)
                    .unwrap();
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => unreachable!(),
        };

        let view = &frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.encode_frame(view, frame.texture.size());
        let dump = self.encode_frame_dump(frame.texture.size());

        self.context.gpu.submit();

        if let Some(dump) = dump {
            self.save_frame_dump(dump);
        }

        self.context.window.pre_present_notify();
        self.context.gpu.queue.present(frame);
        self.context.text_renderer_factory.end_frame();
        self.workspace.end_frame();
    }

    fn encode_frame(&mut self, view: &wgpu::TextureView, size: wgpu::Extent3d) {
        {
            let bg_color = self.context.config.background_color();
            let bg_color = wgpu_jumpstart::Color::from(bg_color).into_linear_wgpu_color();
            let rpass = self
                .context
                .gpu
                .encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Main Neothesia Pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(bg_color),
                            store: wgpu::StoreOp::Store,
                        },
                        depth_slice: None,
                    })],

                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });

            let mut rpass = wgpu_jumpstart::RenderPass::new(rpass, size);

            self.workspace.render(&mut rpass);
        }
    }

    /// Render the frame once more into a texture that can be read back
    fn encode_frame_dump(&mut self, size: wgpu::Extent3d) -> Option<(wgpu::Buffer, u32, u32)> {
        let dump = self.frame_dump.as_mut()?;
        dump.frame += 1;
        if !dump.at.contains(&dump.frame) {
            return None;
        }

        let texture = self
            .context
            .gpu
            .device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("frame dump"),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.context.gpu.texture_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.encode_frame(&view, size);

        let row = (size.width * 4).div_ceil(256) * 256;
        let buffer = self
            .context
            .gpu
            .device
            .create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: (row * size.height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
        self.context.gpu.encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: Default::default(),
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(size.height),
                },
            },
            size,
        );
        Some((buffer, size.width, size.height))
    }

    fn save_frame_dump(&mut self, (buffer, width, height): (wgpu::Buffer, u32, u32)) {
        let Some(dump) = self.frame_dump.as_ref() else {
            return;
        };
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.context
            .gpu
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .ok();
        let Ok(data) = slice.get_mapped_range() else {
            return;
        };
        let row = (width * 4).div_ceil(256) * 256;
        let bgra = matches!(
            self.context.gpu.texture_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            let line = &data[(y * row) as usize..(y * row + width * 4) as usize];
            for px in line.as_chunks::<4>().0 {
                if bgra {
                    rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
                } else {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
            }
        }
        let path = dump.dir.join(format!("frame_{:05}.png", dump.frame));
        match neothesia_image::save_png(&path, &rgba, width, height) {
            Ok(()) => log::info!("Saved {}", path.display()),
            Err(err) => log::error!("{}: {err}", path.display()),
        }
        if dump.at.last() == Some(&dump.frame) {
            self.context.proxy.send_event(NeothesiaEvent::Exit).ok();
        }
    }
}

// This is so stupid, but winit holds us at gunpoint with create_window deprecation
struct NeothesiaBootstrap(Option<Neothesia>, EventLoopProxy<NeothesiaEvent>);

impl ApplicationHandler<NeothesiaEvent> for NeothesiaBootstrap {
    fn resumed(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if self.0.is_some() {
            return;
        }

        let mut attributes = winit::window::Window::default_attributes()
            .with_inner_size(winit::dpi::LogicalSize {
                width: 1080.0,
                height: 720.0,
            })
            .with_title("Neothesia")
            .with_min_inner_size(winit::dpi::LogicalSize {
                width: 670.0,
                height: 620.0,
            })
            .with_theme(Some(winit::window::Theme::Dark));

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::{
                startup_notify::{
                    self, EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
                },
                wayland::WindowAttributesExtWayland,
            };

            if let Some(token) = event_loop.read_token_from_env() {
                startup_notify::reset_activation_token_env();
                attributes = attributes.with_activation_token(token);
            }

            attributes = attributes.with_name("com.github.polymeilex.neothesia", "main");
        };

        let window = event_loop.create_window(attributes).unwrap();

        if let Err(err) = set_window_icon(&window) {
            log::error!("Failed to load window icon: {err}");
        }

        let window_state = WindowState::new(&window);
        let size = window.inner_size();
        let window = Arc::new(window);
        let (gpu, surface) = pollster::block_on(Gpu::for_window(
            || window.clone().into(),
            || Box::new(event_loop.owned_display_handle()),
            size.width,
            size.height,
        ))
        .unwrap();

        let ctx = Context::new(window, window_state, self.1.clone(), gpu);

        let app = Neothesia::new(ctx, surface);
        self.0 = Some(app);
    }

    fn user_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        event: NeothesiaEvent,
    ) {
        if let Some(app) = self.0.as_mut() {
            app.user_event(event_loop, event);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        let Some(app) = self.0.as_mut() else {
            return;
        };

        let mut on_event = |event: WindowEvent| {
            app.window_event(event_loop, window_id, &event);
        };

        // Touch event to mouse event translation (temporary until we get touch support)
        if let WindowEvent::Touch(touch) = &event {
            // TODO: What to do with touch.id? We somehow want to ignore multitouch

            match touch.phase {
                TouchPhase::Started => {
                    // Touch might happen anywhere on the screen, so send moved event
                    on_event(WindowEvent::CursorMoved {
                        device_id: touch.device_id,
                        position: touch.location,
                    });
                    on_event(WindowEvent::MouseInput {
                        device_id: touch.device_id,
                        state: ElementState::Pressed,
                        button: MouseButton::Left,
                    });
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    on_event(WindowEvent::MouseInput {
                        device_id: touch.device_id,
                        state: ElementState::Released,
                        button: MouseButton::Left,
                    });
                }
                TouchPhase::Moved => {
                    on_event(WindowEvent::CursorMoved {
                        device_id: touch.device_id,
                        position: touch.location,
                    });
                }
            }
        } else {
            app.window_event(event_loop, window_id, &event)
        }
    }

    fn about_to_wait(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        if let Some(app) = self.0.as_mut() {
            app.about_to_wait(event_loop)
        }
    }
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info, wgpu_hal=error, oxisynth=error"),
    )
    .init();

    #[cfg(feature = "profiling-on")]
    puffin::set_scopes_on(true); // tell puffin to collect data
    #[cfg(feature = "profiling-on")]
    let _server = puffin_http::Server::new("127.0.0.1:8585").ok();

    let event_loop: EventLoop<NeothesiaEvent> = EventLoop::with_user_event().build().unwrap();
    let proxy = event_loop.create_proxy();

    event_loop
        .run_app(&mut NeothesiaBootstrap(None, proxy))
        .unwrap();
}

fn set_window_icon(window: &winit::window::Window) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Cursor;

    let (icon, w, h) = neothesia_image::load_png(Cursor::new(include_bytes!(
        "../../flatpak/com.github.polymeilex.neothesia.png"
    )))?;

    window.set_window_icon(Some(winit::window::Icon::from_rgba(icon, w, h)?));

    Ok(())
}
