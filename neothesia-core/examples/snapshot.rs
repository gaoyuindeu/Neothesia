//! Render frames of a song to PNG files without opening a window, using the same
//! renderers and draw order as the playing scene. Useful for checking visuals.
//!
//! cargo run --release -p neothesia-core --example snapshot -- <midi> <out_dir> <sec>... [--bg <image>]

use std::{path::PathBuf, time::Duration};

use neothesia_core::{
    config::Config,
    piano_layout,
    render::{
        Backdrop, FxKey, FxRenderer, GuidelineRenderer, KeyboardRenderer, QuadRendererFactory,
        SheetColors, SheetRenderer, TextRendererFactory, WaterfallRenderer,
    },
};
use wgpu_jumpstart::{Gpu, TransformUniform, Uniform, wgpu};

const WIDTH: u32 = 1920;
const HEIGHT: u32 = 1080;
const LEAD_IN: Duration = Duration::from_secs(3);

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let bg = args.iter().position(|a| a == "--bg").map(|i| {
        let path = PathBuf::from(args.remove(i + 1));
        args.remove(i);
        path
    });
    let midi_path = PathBuf::from(&args[0]);
    let out_dir = PathBuf::from(&args[1]);
    let mut times: Vec<f32> = args[2..].iter().map(|t| t.parse().unwrap()).collect();
    times.sort_by(f32::total_cmp);
    std::fs::create_dir_all(&out_dir).unwrap();

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let mut gpu = pollster::block_on(Gpu::new(instance, None)).expect("GPU init");
    let midi = midi_file::MidiFile::new(&midi_path).expect("MIDI");

    let mut config = Config::new();
    if bg.is_some() {
        config.set_background_image(bg);
    }

    let mut transform = TransformUniform::default();
    transform.update(WIDTH as f32, HEIGHT as f32, 1.0);
    let transform = Uniform::new(
        &gpu.device,
        transform,
        wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
    );

    let quads = QuadRendererFactory::new(&gpu, &transform);
    let mut quad_bg = quads.new_renderer();
    let mut quad_fg = quads.new_renderer();
    let mut text = TextRendererFactory::new(&gpu).new_renderer();

    let range = piano_layout::KeyboardRange::new(config.piano_range());
    let layout = piano_layout::KeyboardLayout::from_range(
        piano_layout::Sizing::new(
            WIDTH as f32 / range.white_count() as f32,
            HEIGHT as f32 * 0.2,
        ),
        range,
    );
    let mut keyboard = KeyboardRenderer::new(layout.clone());
    keyboard.position_on_bottom_of_parent(HEIGHT as f32);

    let mut guidelines = GuidelineRenderer::new(
        layout.clone(),
        *keyboard.pos(),
        config.vertical_guidelines(),
        config.horizontal_guidelines(),
        midi.measures.clone(),
    );
    let mut waterfall =
        WaterfallRenderer::new(&gpu, &midi.tracks, &[], &config, &transform, layout.clone());
    let mut fx = FxRenderer::new(&gpu, &transform, layout.range.iter().count());
    let mut backdrop = Backdrop::new(
        &gpu,
        config.background_image(),
        config.background_color(),
        config.background_dim(),
        config.background_blur(),
    );

    let score = midi_file::score::Score::new(&midi);
    println!(
        "score: alignment {:.2}, readable {}",
        score.grid_alignment,
        score.is_readable()
    );
    let text_factory = TextRendererFactory::new(&gpu);
    let mut sheet = score.is_readable().then(|| {
        SheetRenderer::new(
            score,
            quads.new_renderer(),
            text_factory.new_renderer(),
            SheetColors {
                background: [0.008, 0.008, 0.016, 1.0],
                ink: [225, 225, 235],
                played: [120, 120, 135],
                hands: [[90, 255, 140], [110, 190, 255]],
            },
        )
    });

    let mut playback = midi_file::PlaybackState::new(LEAD_IN, midi.tracks.clone());

    let texture_desc = wgpu::TextureDescriptor {
        size: wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        view_formats: &[],
        usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::RENDER_ATTACHMENT,
        label: None,
    };
    let texture = gpu.device.create_texture(&texture_desc);
    let view = texture.create_view(&Default::default());

    let frame = Duration::from_secs(1) / 60;
    let mut elapsed = 0.0f32;

    for target in times {
        // Simulate every frame up to the target, so particles evolve naturally
        while elapsed < target {
            elapsed += frame.as_secs_f32();
            let events = playback.update(frame);
            for e in events {
                let (on, key) = match e.message {
                    midi_file::midly::MidiMessage::NoteOn { key, .. } => (true, key.as_int()),
                    midi_file::midly::MidiMessage::NoteOff { key, .. } => (false, key.as_int()),
                    _ => continue,
                };
                if !keyboard.range().contains(key) || e.channel == 9 {
                    continue;
                }
                let id = key as usize - keyboard.range().start() as usize;
                let state = &mut keyboard.key_states_mut()[id];
                if on {
                    let schema = config.color_schema();
                    state.pressed_by_file_on(&schema[e.track_color_id % schema.len()]);
                } else {
                    state.pressed_by_file_off();
                }
                keyboard.invalidate_cache();
            }

            let pos = *keyboard.pos();
            let pressed: Vec<FxKey> = keyboard
                .layout()
                .keys
                .iter()
                .zip(keyboard.key_states())
                .filter_map(|(key, state)| {
                    Some(FxKey {
                        id: key.id(),
                        x: pos.x + key.x(),
                        width: key.width(),
                        color: *state.pressed_by_file()?,
                    })
                })
                .collect();
            fx.update(frame, &pressed, pos.x, layout.width, pos.y);
            backdrop.update(frame, (WIDTH as f32, HEIGHT as f32));
        }

        let time = playback.time().as_secs_f32() - playback.leed_in().as_secs_f32();
        quad_bg.clear();
        quad_fg.clear();
        guidelines.update(
            &mut quad_bg,
            config.animation_speed(),
            1.0,
            time,
            neothesia_core::dpi::LogicalSize::new(WIDTH as f32, HEIGHT as f32),
        );
        waterfall.update(time);
        keyboard.update(&mut quad_fg, &mut text);
        quad_bg.prepare();
        quad_fg.prepare();
        fx.prepare();
        text.update(neothesia_core::dpi::PhysicalSize::new(WIDTH, HEIGHT), 1.0);
        if let Some(sheet) = sheet.as_mut() {
            let song_time = playback.time().saturating_sub(*playback.leed_in());
            sheet.update(
                song_time,
                (0.0, 0.0, WIDTH as f32, SheetRenderer::height_for(13.0)),
                neothesia_core::dpi::PhysicalSize::new(WIDTH, HEIGHT),
                1.0,
            );
        }

        {
            let rpass = gpu.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut rpass = wgpu_jumpstart::RenderPass::new(rpass, texture.size());
            backdrop.render(&mut rpass);
            quad_bg.render(&mut rpass);
            waterfall.render(&mut rpass);
            quad_fg.render(&mut rpass);
            fx.render(&mut rpass);
            text.render(&mut rpass);
            if let Some(sheet) = sheet.as_ref() {
                sheet.render(&mut rpass);
            }
        }

        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            size: (WIDTH * HEIGHT * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            label: None,
            mapped_at_creation: false,
        });
        gpu.encoder.copy_texture_to_buffer(
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
                    bytes_per_row: Some(WIDTH * 4),
                    rows_per_image: Some(HEIGHT),
                },
            },
            texture_desc.size,
        );
        gpu.submit();

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        gpu.device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .unwrap();
        let mut rgba = slice.get_mapped_range().unwrap().to_vec();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
        }

        let path = out_dir.join(format!("frame_{:07.2}.png", target));
        image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba)
            .unwrap()
            .save(&path)
            .unwrap();
        println!("{}", path.display());
    }
}
