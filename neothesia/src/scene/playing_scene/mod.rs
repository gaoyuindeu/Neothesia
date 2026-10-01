use midi_file::midly::MidiMessage;
use neothesia_core::render::{
    Backdrop, FxKey, FxRenderer, GuidelineRenderer, LabelOptions, NoteLabels, QuadRenderer,
    SheetColors, SheetRenderer, TextRenderer,
};
use std::time::Duration;
use winit::{
    event::WindowEvent,
    keyboard::{Key, NamedKey},
};

use self::top_bar::TopBar;

use super::{NuonRenderer, Scene};
use crate::{
    NeothesiaEvent, context::Context, render::WaterfallRenderer, scene::MouseToMidiEventState,
    song::Song, utils::window::WinitEvent,
};

mod keyboard;
pub use keyboard::Keyboard;

pub(crate) mod midi_player;
use midi_player::MidiPlayer;

mod rewind_controller;
use rewind_controller::RewindController;

mod step_controller;
use step_controller::StepController;

mod toast_manager;
use toast_manager::ToastManager;

mod animation;
mod top_bar;

pub struct PlayingScene {
    keyboard: Keyboard,
    waterfall: WaterfallRenderer,
    guidelines: GuidelineRenderer,
    text_renderer: TextRenderer,
    nuon_renderer: NuonRenderer,

    note_labels: Option<NoteLabels>,

    player: MidiPlayer,
    rewind_controller: RewindController,
    step_controller: StepController,
    quad_renderer_bg: QuadRenderer,
    quad_renderer_fg: QuadRenderer,
    fx: Option<FxRenderer>,
    backdrop: Backdrop,
    /// None when the song is not score-like (e.g. a recorded performance)
    sheet: Option<SheetRenderer>,
    show_sheet: bool,
    toast_manager: ToastManager,

    nuon: nuon::Ui,
    mouse_to_midi_state: MouseToMidiEventState,

    deduced_chord_name: String,

    top_bar: TopBar,
}

impl PlayingScene {
    /// Stop at the current position (used when a song is opened without starting it)
    pub fn pause(&mut self) {
        self.player.pause();
    }

    /// Position in the song (with the lead-in), and whether it is paused: to rebuild the
    /// scene where it was
    pub fn position(&self) -> (std::time::Duration, bool) {
        (self.player.time(), self.player.is_paused())
    }

    pub fn seek(&mut self, time: std::time::Duration) {
        self.player.set_time(time);
        self.keyboard.reset_notes();
    }

    pub fn new(ctx: &mut Context, song: Song) -> Self {
        let keyboard = Keyboard::new(ctx, song.config.clone());

        let keyboard_layout = keyboard.layout();

        let guidelines = GuidelineRenderer::new(
            keyboard_layout.clone(),
            *keyboard.pos(),
            ctx.config.vertical_guidelines(),
            ctx.config.horizontal_guidelines(),
            song.file.measures.clone(),
        );

        let hidden_tracks: Vec<usize> = song
            .config
            .tracks
            .iter()
            .filter(|t| !t.visible)
            .map(|t| t.track_id)
            .collect();

        let mut waterfall = WaterfallRenderer::new(
            &ctx.gpu,
            &song.file.tracks,
            &hidden_tracks,
            &ctx.config,
            &ctx.transform,
            keyboard_layout.clone(),
        );

        let text_renderer = ctx.text_renderer_factory.new_renderer();

        let label_options = LabelOptions {
            names: ctx.config.note_labels(),
            printed_fingers: ctx.config.fingering(),
            estimated_fingers: ctx.config.fingering() && ctx.config.estimated_fingering(),
        };
        let note_labels = label_options.any().then(|| {
            NoteLabels::new(
                *keyboard.pos(),
                waterfall.notes(),
                ctx.text_renderer_factory.new_renderer(),
                label_options,
            )
        });

        let player = MidiPlayer::new(
            ctx.output_manager.connection().clone(),
            song,
            keyboard_layout.range.clone(),
            ctx.config.separate_channels(),
        );
        waterfall.update(player.time_without_lead_in());

        let step_controller = StepController::new(player.song(), *player.leed_in());

        let quad_renderer_bg = ctx.quad_renderer_factory.new_renderer();
        let quad_renderer_fg = ctx.quad_renderer_factory.new_renderer();

        let fx = ctx.config.glow().then(|| {
            FxRenderer::new(
                &ctx.gpu,
                &ctx.transform,
                keyboard.layout().range.iter().count(),
            )
        });

        let sheet = {
            let mut score = midi_file::score::Score::new(&player.song().file);
            midi_file::fingering::retain(
                &mut score,
                ctx.config.fingering(),
                ctx.config.fingering() && ctx.config.estimated_fingering(),
            );
            log::info!(
                "Sheet music: grid alignment {:.2}, readable {}",
                score.grid_alignment,
                score.is_readable()
            );
            let hand_color = |hand: midi_file::Hand| {
                let schema = ctx.config.color_schema();
                let (r, g, b) = schema[hand.color_id() % schema.len()].base;
                [r, g, b]
            };
            score.is_readable().then(|| {
                SheetRenderer::new(
                    score,
                    ctx.quad_renderer_factory.new_renderer(),
                    ctx.text_renderer_factory.new_renderer(),
                    // Paper: black notes on white, as printed
                    SheetColors {
                        background: [1.0, 1.0, 1.0, 1.0],
                        ink: [0, 0, 0],
                        played: [150, 150, 150],
                        hands: [
                            hand_color(midi_file::Hand::Right),
                            hand_color(midi_file::Hand::Left),
                        ],
                    },
                )
            })
        };

        let backdrop = Backdrop::new(
            &ctx.gpu,
            ctx.config.background_image(),
            ctx.config.background_color(),
            ctx.config.background_dim(),
            ctx.config.background_blur(),
        );

        Self {
            keyboard,
            guidelines,
            note_labels,
            text_renderer,
            nuon_renderer: NuonRenderer::new(ctx),

            waterfall,
            player,
            rewind_controller: RewindController::new(),
            step_controller,
            quad_renderer_bg,
            quad_renderer_fg,
            fx,
            backdrop,
            sheet,
            show_sheet: true,
            toast_manager: ToastManager::default(),

            nuon: nuon::Ui::new(),
            mouse_to_midi_state: MouseToMidiEventState::default(),
            deduced_chord_name: String::new(),

            top_bar: TopBar::new(),
        }
    }

    fn update_fx(&mut self, delta: Duration) {
        let Some(fx) = &mut self.fx else {
            return;
        };

        let layout = self.keyboard.layout();
        let pos = *self.keyboard.pos();

        let pressed: Vec<FxKey> = layout
            .keys
            .iter()
            .zip(self.keyboard.key_states())
            .filter_map(|(key, state)| {
                let color = state.pressed_by_file().or(state.pressed_by_user())?;
                Some(FxKey {
                    id: key.id(),
                    x: pos.x + key.x(),
                    width: key.width(),
                    color: *color,
                })
            })
            .collect();

        fx.update(delta, &pressed, pos.x, layout.width, pos.y);
    }

    fn update_chord_identifier(&mut self, enabled: bool) {
        if !enabled {
            return;
        }

        let start = self.keyboard.layout().range.start();
        let notes = self
            .keyboard
            .key_states()
            .iter()
            .enumerate()
            .filter(|(_, state)| state.pressed_by_user().is_some())
            .map(|(id, _)| id as u8 + start)
            .collect::<Vec<_>>();

        self.deduced_chord_name = super::freeplay::chords::deduce_name(&notes).unwrap_or_default();
    }

    #[profiling::function]
    fn update_midi_player(&mut self, ctx: &Context, delta: Duration) -> f32 {
        if self.top_bar.is_looper_active() && self.player.time() > self.top_bar.loop_end_timestamp()
        {
            self.player.set_time(self.top_bar.loop_start_timestamp());
            self.keyboard.reset_notes();
            self.step_controller.cancel_glide();
        }

        if self.step_controller.is_enabled() {
            let advance = self.step_controller.update(&self.player, delta);
            if !advance.is_zero() {
                let midi_events = self.player.step_update(advance);
                let midi_events: Vec<_> = midi_events.iter().collect();
                self.keyboard.file_midi_events(&ctx.config, &midi_events);
            }
        } else if self.player.play_along().are_required_keys_pressed() {
            let delta = delta.mul_f32(ctx.config.speed_multiplier());
            let midi_events = self.player.update(delta);
            self.keyboard.file_midi_events(&ctx.config, &midi_events);
        }

        self.player.time_without_lead_in() + ctx.config.animation_offset()
    }

    #[profiling::function]
    fn resize(&mut self, ctx: &mut Context) {
        self.keyboard.resize(ctx);

        self.guidelines.set_layout(self.keyboard.layout().clone());
        self.guidelines.set_pos(*self.keyboard.pos());
        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.set_pos(*self.keyboard.pos());
        }

        self.waterfall
            .resize(&ctx.config, self.keyboard.layout().clone());
    }
}

impl Scene for PlayingScene {
    #[profiling::function]
    fn update(&mut self, ctx: &mut Context, delta: Duration) {
        self.quad_renderer_bg.clear();
        self.quad_renderer_fg.clear();

        self.rewind_controller.update(&mut self.player, ctx, delta);
        self.toast_manager.update(&mut self.text_renderer);

        let time = self.update_midi_player(ctx, delta);
        self.waterfall.update(time);
        self.guidelines.update(
            &mut self.quad_renderer_bg,
            ctx.config.animation_speed(),
            ctx.window_state.scale_factor as f32,
            time,
            ctx.window_state.logical_size,
        );
        self.keyboard
            .update(&mut self.quad_renderer_fg, &mut self.text_renderer);
        self.update_chord_identifier(ctx.config.chord_identifier());
        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.update(
                ctx.window_state.physical_size,
                ctx.window_state.scale_factor as f32,
                self.keyboard.renderer(),
                ctx.config.animation_speed(),
                time,
            );
        }

        self.update_fx(delta);
        self.show_sheet = ctx.config.sheet_music();
        if let Some(sheet) = self.sheet.as_mut().filter(|_| ctx.config.sheet_music()) {
            let song_time = self.player.time().saturating_sub(*self.player.leed_in());
            let logical = ctx.window_state.logical_size;
            let staff_space = (logical.height * 0.012).clamp(8.0, 14.0);
            sheet.update(
                song_time,
                (
                    0.0,
                    0.0,
                    logical.width,
                    SheetRenderer::height_for(staff_space),
                ),
                ctx.window_state.physical_size,
                ctx.window_state.scale_factor as f32,
            );
        }
        let size = ctx.window_state.physical_size;
        self.backdrop
            .update(delta, (size.width as f32, size.height as f32));

        TopBar::update(self, ctx);

        if ctx.config.chord_identifier() {
            nuon::label()
                .text(&self.deduced_chord_name)
                .font_size(25.0)
                .y(self.keyboard.pos().y - 35.0)
                .height(25.0)
                .width(ctx.window_state.logical_size.width)
                .build(&mut self.nuon);
        }

        super::render_nuon(&mut self.nuon, &mut self.nuon_renderer, ctx);

        self.quad_renderer_bg.prepare();
        self.quad_renderer_fg.prepare();

        if let Some(fx) = &mut self.fx {
            fx.prepare();
        }

        #[cfg(debug_assertions)]
        self.text_renderer.queue_fps(
            ctx.fps_ticker.avg(),
            self.top_bar
                .topbar_expand_animation
                .animate_bool(5.0, 80.0, ctx.frame_timestamp),
        );
        self.text_renderer.update(
            ctx.window_state.physical_size,
            ctx.window_state.scale_factor as f32,
        );

        if self.player.is_finished() && !self.player.is_paused() {
            // Back to the start, paused, ready for another go
            self.player.pause();
            self.player.set_percentage_time(0.0);
            self.keyboard.reset_notes();
            ctx.proxy
                .send_event(NeothesiaEvent::MainMenu(Some(self.player.song().clone())))
                .ok();
        }
    }

    #[profiling::function]
    fn render<'pass>(&'pass mut self, rpass: &mut wgpu_jumpstart::RenderPass<'pass>) {
        self.backdrop.render(rpass);
        self.quad_renderer_bg.render(rpass);
        self.waterfall.render(rpass);
        if let Some(note_labels) = self.note_labels.as_mut() {
            note_labels.render(rpass);
        }
        self.quad_renderer_fg.render(rpass);
        if let Some(fx) = &self.fx {
            fx.render(rpass);
        }
        self.text_renderer.render(rpass);

        if let Some(sheet) = self.sheet.as_ref().filter(|_| self.show_sheet) {
            sheet.render(rpass);
        }

        self.nuon_renderer.render(rpass);
    }

    fn window_event(&mut self, ctx: &mut Context, event: &WindowEvent) {
        if event.key_pressed(Key::Named(NamedKey::Tab)) {
            self.step_controller.toggle(&mut self.player);
            self.keyboard.reset_notes();
            self.toast_manager
                .toast(if self.step_controller.is_enabled() {
                    "Step Mode: \u{2192} next, \u{2190} previous"
                } else {
                    "Step Mode: off"
                });
        }

        let step_before = self.player.time();
        if self
            .step_controller
            .handle_window_event(event, &mut self.player)
        {
            if self.player.time() < step_before {
                self.keyboard.reset_notes();
            }
        } else {
            self.rewind_controller
                .handle_window_event(ctx, event, &mut self.player);
        }

        if self.rewind_controller.is_rewinding() {
            self.step_controller.cancel_glide();
            self.keyboard.reset_notes();
        }

        if event.back_mouse_pressed() || event.key_released(Key::Named(NamedKey::Escape)) {
            ctx.proxy
                .send_event(NeothesiaEvent::MainMenu(Some(self.player.song().clone())))
                .ok();
        }

        if event.key_released(Key::Named(NamedKey::Space)) {
            if self.step_controller.is_enabled() {
                self.step_controller.set_enabled(false, &mut self.player);
                self.toast_manager.toast("Step Mode: off");
                self.player.resume();
            } else {
                self.player.pause_resume();
            }
        }

        handle_settings_input(ctx, &mut self.toast_manager, &mut self.waterfall, event);
        super::handle_pc_keyboard_to_midi_event(ctx, event);
        super::handle_mouse_to_midi_event(
            &mut self.keyboard,
            &mut self.mouse_to_midi_state,
            ctx,
            event,
        );

        if event.window_resized() || event.scale_factor_changed() {
            self.resize(ctx)
        }

        super::handle_nuon_window_event(&mut self.nuon, event, ctx);
    }

    fn midi_event(&mut self, _ctx: &mut Context, channel: u8, message: &MidiMessage) {
        self.player.user_midi_event(channel, message);
        self.keyboard.user_midi_event(message);
    }
}

fn handle_settings_input(
    ctx: &mut Context,
    toast_manager: &mut ToastManager,
    waterfall: &mut WaterfallRenderer,
    event: &WindowEvent,
) {
    if event.key_released(Key::Named(NamedKey::ArrowUp))
        || event.key_released(Key::Named(NamedKey::ArrowDown))
    {
        let amount = if ctx.window_state.modifiers_state.shift_key() {
            0.5
        } else {
            0.1
        };

        if event.key_released(Key::Named(NamedKey::ArrowUp)) {
            ctx.config
                .set_speed_multiplier(ctx.config.speed_multiplier() + amount);
        } else {
            ctx.config
                .set_speed_multiplier((ctx.config.speed_multiplier() - amount).max(0.05));
        }

        toast_manager.speed_toast(ctx.config.speed_multiplier());
        return;
    }

    if event.key_released(Key::Named(NamedKey::PageUp))
        || event.key_released(Key::Named(NamedKey::PageDown))
    {
        let amount = if ctx.window_state.modifiers_state.shift_key() {
            500.0
        } else {
            100.0
        };

        if event.key_released(Key::Named(NamedKey::PageUp)) {
            ctx.config
                .set_animation_speed(ctx.config.animation_speed() + amount);
        } else {
            ctx.config
                .set_animation_speed(ctx.config.animation_speed() - amount);
        }

        waterfall
            .pipeline()
            .set_speed(&ctx.gpu.queue, ctx.config.animation_speed());
        toast_manager.animation_speed_toast(ctx.config.animation_speed());
        return;
    }

    if let Some(ch @ ("_" | "-" | "+" | "=")) = event.character_released() {
        let amount = if ctx.window_state.modifiers_state.shift_key() {
            0.1
        } else {
            0.01
        };

        if matches!(ch, "-" | "_") {
            ctx.config
                .set_animation_offset(ctx.config.animation_offset() - amount);
        } else {
            ctx.config
                .set_animation_offset(ctx.config.animation_offset() + amount);
        }

        toast_manager.offset_toast(ctx.config.animation_offset());
    }
}
