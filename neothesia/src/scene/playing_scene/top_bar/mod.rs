use std::time::Instant;

use crate::{NeothesiaEvent, context::Context, icons};

use super::{
    PlayingScene,
    animation::{Animated, Easing},
};

/// The row of buttons
const CONTROLS_H: f32 = 30.0;
/// The progress bar under it
const PROGRESS_H: f32 = 24.0;
/// The bar is always shown: what is under it starts here
pub const HEIGHT: f32 = CONTROLS_H + PROGRESS_H;

const BAR_BG: [u8; 3] = [37, 35, 42];
const PILL: [u8; 3] = [67, 67, 67];
const PILL_HOVER: [u8; 3] = [87, 87, 87];
const PILL_PRESSED: [u8; 3] = [97, 97, 97];
const CHIP: [u8; 3] = [52, 50, 58];
const PROGRESS: [u8; 3] = [56, 145, 255];
const LOOP: [u8; 3] = [255, 56, 187];

/// Practice speeds, one click each beside the speed control
const SPEED_PRESETS: [f32; 7] = [0.25, 0.5, 0.7, 1.0, 1.2, 1.5, 2.0];

pub struct TopBar {
    settings_animation: Animated<bool, Instant>,
    settings_active: bool,
}

impl TopBar {
    pub fn new() -> Self {
        Self {
            settings_animation: Animated::new(false)
                .duration(1000.)
                .easing(Easing::EaseOutExpo)
                .delay(30.0),
            settings_active: false,
        }
    }

    #[profiling::function]
    pub fn update(scene: &mut PlayingScene, ctx: &mut Context) {
        scene
            .top_bar
            .settings_animation
            .transition(scene.top_bar.settings_active, ctx.frame_timestamp);

        let mut ui = std::mem::replace(&mut scene.nuon, nuon::Ui::new());
        Self::panel(scene, ctx, &mut ui);
        scene.nuon = ui;
    }

    fn panel(this: &mut PlayingScene, ctx: &mut Context, ui: &mut nuon::Ui) {
        let win_w = ctx.window_state.logical_size.width;

        nuon::quad().size(win_w, HEIGHT).color(BAR_BG).build(ui);

        Self::panel_left(this, ctx, ui);
        Self::panel_center(ctx, ui);
        Self::panel_right(this, ctx, ui);
        Self::settings_panel(this, ctx, ui);

        nuon::translate().y(CONTROLS_H).build(ui, |ui| {
            Self::progress_bar(this, ctx, ui);
        });
    }

    fn button() -> nuon::Button {
        nuon::button()
            .size(CONTROLS_H, CONTROLS_H)
            .border_radius([5.0; 4])
    }

    fn panel_left(this: &mut PlayingScene, ctx: &mut Context, ui: &mut nuon::Ui) {
        if Self::button().icon(icons::left_arrow_icon()).build(ui) {
            ctx.proxy
                .send_event(NeothesiaEvent::MainMenu(Some(this.player.song().clone())))
                .ok();
        }
    }

    /// The speed: − / percentage / +, and the preset speeds beside it
    fn panel_center(ctx: &mut Context, ui: &mut nuon::Ui) {
        const BUTTON_W: f32 = 32.0;
        const LABEL_W: f32 = 56.0;
        const CHIP_W: f32 = 40.0;
        const CHIP_GAP: f32 = 3.0;
        const GROUP_GAP: f32 = 10.0;
        const H: f32 = 20.0;

        let win_w = ctx.window_state.logical_size.width;
        let pill_w = BUTTON_W * 2.0 + LABEL_W;
        let chips_w =
            SPEED_PRESETS.len() as f32 * CHIP_W + (SPEED_PRESETS.len() - 1) as f32 * CHIP_GAP;
        // Between the back button and the three buttons on the right; centered when it fits
        let left = CONTROLS_H + 10.0;
        let right = win_w - 3.0 * CONTROLS_H - 10.0;
        let show_chips = pill_w + GROUP_GAP + chips_w <= right - left;
        let group_w = if show_chips {
            pill_w + GROUP_GAP + chips_w
        } else {
            pill_w
        };
        let group_x = (win_w / 2.0 - group_w / 2.0)
            .min(right - group_w)
            .max(left)
            .round();

        let speed = ctx.config.speed_multiplier();

        nuon::translate()
            .x(group_x)
            .y((CONTROLS_H - H) / 2.0)
            .build(ui, |ui| {
                if nuon::button()
                    .size(BUTTON_W, H)
                    .color(PILL)
                    .hover_color(PILL_HOVER)
                    .preseed_color(PILL_PRESSED)
                    .border_radius([10.0, 0.0, 0.0, 10.0])
                    .icon(icons::minus_icon())
                    .build(ui)
                {
                    ctx.config.set_speed_multiplier((speed - 0.05).max(0.05));
                }

                // Clicking the percentage goes to the next preset
                let presets = nuon::click_area("SpeedPresets")
                    .x(BUTTON_W)
                    .size(LABEL_W, H)
                    .build(ui);
                if presets.is_clicked() {
                    ctx.config.set_speed_multiplier(next_speed_preset(speed));
                }
                nuon::quad()
                    .x(BUTTON_W)
                    .size(LABEL_W, H)
                    .color(if presets.is_hovered() || presets.is_pressed() {
                        PILL_HOVER
                    } else {
                        PILL
                    })
                    .build(ui);
                nuon::label()
                    .text(format_speed(speed))
                    .bold(true)
                    .x(BUTTON_W)
                    .size(LABEL_W, H)
                    .build(ui);

                if nuon::button()
                    .size(BUTTON_W, H)
                    .x(BUTTON_W + LABEL_W)
                    .color(PILL)
                    .hover_color(PILL_HOVER)
                    .preseed_color(PILL_PRESSED)
                    .border_radius([0.0, 10.0, 10.0, 0.0])
                    .icon(icons::plus_icon())
                    .build(ui)
                {
                    ctx.config.set_speed_multiplier(speed + 0.05);
                }

                if !show_chips {
                    return;
                }
                for (i, preset) in SPEED_PRESETS.into_iter().enumerate() {
                    let x = pill_w + GROUP_GAP + i as f32 * (CHIP_W + CHIP_GAP);
                    let chip = nuon::click_area(("SpeedPreset", i))
                        .x(x)
                        .size(CHIP_W, H)
                        .build(ui);
                    let selected = (speed - preset).abs() < 0.001;
                    let color = if selected {
                        PROGRESS
                    } else if chip.is_pressed() {
                        PILL_PRESSED
                    } else if chip.is_hovered() {
                        PILL_HOVER
                    } else {
                        CHIP
                    };
                    nuon::quad()
                        .x(x)
                        .size(CHIP_W, H)
                        .color(color)
                        .border_radius([10.0; 4])
                        .build(ui);
                    nuon::label()
                        .text(format_speed(preset))
                        .bold(selected)
                        .font_size(12.0)
                        .x(x)
                        .size(CHIP_W, H)
                        .build(ui);
                    if chip.is_clicked() {
                        ctx.config.set_speed_multiplier(preset);
                    }
                }
            });
    }

    fn panel_right(this: &mut PlayingScene, ctx: &mut Context, ui: &mut nuon::Ui) {
        nuon::translate()
            .x(ctx.window_state.logical_size.width)
            .build(ui, |ui| {
                nuon::translate().x(-CONTROLS_H).add_to_current(ui);

                if Self::button()
                    .icon(if this.top_bar.settings_active {
                        icons::gear_fill_icon()
                    } else {
                        icons::gear_icon()
                    })
                    .build(ui)
                {
                    this.top_bar.settings_active = !this.top_bar.settings_active;
                }

                nuon::translate().x(-CONTROLS_H).add_to_current(ui);

                // A–B repeat on / off (A and B keys mark the section)
                let mut repeat = Self::button().icon(icons::repeat_icon());
                if this.looper.is_active() {
                    repeat = repeat
                        .color([120, 36, 92])
                        .hover_color([150, 46, 116])
                        .preseed_color([170, 52, 132]);
                }
                if repeat.build(ui) {
                    let now = this.player.time();
                    this.looper.toggle(now, this.player.length());
                    if let (true, Some(start), Some(end)) = (
                        this.looper.is_active(),
                        this.looper.start(),
                        this.looper.end(),
                    ) && !(start..=end).contains(&now)
                    {
                        this.jump_to(start);
                    }
                    this.looper.save(ctx);
                }

                nuon::translate().x(-CONTROLS_H).add_to_current(ui);

                if Self::button()
                    .icon(if this.player.is_paused() {
                        icons::play_icon()
                    } else {
                        icons::pause_icon()
                    })
                    .build(ui)
                {
                    if this.step_controller.is_enabled() {
                        this.step_controller.set_enabled(false, &mut this.player);
                        this.player.resume();
                    } else {
                        this.player.pause_resume();
                    }
                }
            });
    }

    fn settings_panel(this: &mut PlayingScene, ctx: &mut Context, ui: &mut nuon::Ui) {
        let width = 280.0;
        let offset = this
            .top_bar
            .settings_animation
            .animate_bool(0.0, width, ctx.frame_timestamp);

        nuon::translate()
            .x(ctx.window_state.logical_size.width - offset)
            .y(HEIGHT)
            .build(ui, |ui| {
                // Gap
                nuon::translate().y(5.0).add_to_current(ui);

                nuon::quad()
                    .size(width, 208.0)
                    .color(BAR_BG)
                    .border_radius([10.0, 0.0, 0.0, 10.0])
                    .build(ui);

                nuon::translate().x(15.0).build(ui, |ui| {
                    nuon::settings_section("Display").width(width - 30.0).build(
                        ui,
                        |ui, rows, _| {
                            if nuon::settings_row_toggler()
                                .title("Chord Identifier")
                                .subtitle("Display chord above keyboard")
                                .value(ctx.config.chord_identifier())
                                .build(ui, rows)
                            {
                                ctx.config
                                    .set_chord_identifier(!ctx.config.chord_identifier());
                            }

                            if nuon::settings_row_toggler()
                                .title("Sheet Music")
                                .subtitle("Staff above the waterfall")
                                .value(ctx.config.sheet_music())
                                .build(ui, rows)
                            {
                                ctx.config.set_sheet_music(!ctx.config.sheet_music());
                            }

                            if nuon::settings_row_toggler()
                                .title("Step Mode (Tab)")
                                .subtitle("\u{2192} next note, \u{2190} previous")
                                .value(this.step_controller.is_enabled())
                                .build(ui, rows)
                            {
                                this.step_controller.toggle(&mut this.player);
                                this.keyboard.reset_notes();
                            }
                        },
                    );
                });
            });
    }

    fn progress_bar(this: &mut PlayingScene, ctx: &mut Context, ui: &mut nuon::Ui) {
        let h = PROGRESS_H;
        let w = ctx.window_state.logical_size.width;

        // The A–B handles sit on the bar: they take the mouse before it does
        let render_looper = Self::progress_bar_looper(this, ctx, ui, w, h);

        Self::progress_bar_bg(this, ctx, ui, w, h);

        render_looper(ui);
    }

    fn progress_bar_bg(
        this: &mut PlayingScene,
        ctx: &mut Context,
        ui: &mut nuon::Ui,
        w: f32,
        h: f32,
    ) {
        let progress_w = w * this.player.percentage();

        match nuon::click_area("ProggressBar").size(w, h).build(ui) {
            nuon::ClickAreaEvent::PressStart { .. } => {
                if !this.rewind_controller.is_rewinding() {
                    this.rewind_controller.start_mouse_rewind(&mut this.player);

                    let x = ctx.window_state.cursor_logical_position.x;
                    let w = ctx.window_state.logical_size.width;

                    let p = x / w;
                    this.player.set_percentage_time(p);
                    this.keyboard.reset_notes();
                }
            }
            nuon::ClickAreaEvent::PressEnd { .. } => {
                this.rewind_controller.stop_rewind(&mut this.player);
            }
            nuon::ClickAreaEvent::Idle { .. } => {}
        }

        nuon::quad().size(progress_w, h).color(PROGRESS).build(ui);

        let length = this.player.length().as_secs_f32();
        let start = this.player.leed_in().as_secs_f32() / length;
        for m in this.player.song().file.measures.iter() {
            let x = (start + m.as_secs_f32() / length) * w;

            let light_measure = nuon::Color::new(1.0, 1.0, 1.0, 0.5);
            let dark_measure = nuon::Color::new(0.4, 0.4, 0.4, 1.0);

            let color = if x < progress_w {
                light_measure
            } else {
                dark_measure
            };

            nuon::quad().x(x).size(1.0, h).color(color).build(ui);
        }
    }

    /// The A and B marks: draggable, with the section between them tinted (strongly while
    /// it repeats)
    fn progress_bar_looper<'a>(
        this: &mut PlayingScene,
        ctx: &mut Context,
        ui: &mut nuon::Ui,
        w: f32,
        h: f32,
    ) -> impl FnOnce(&mut nuon::Ui) + 'a {
        const GRAB_W: f32 = 12.0;
        const LINE_W: f32 = 3.0;

        let to_x = |time| this.player.time_to_percentage(&time) * w;
        let start_x = this.looper.start().map(to_x);
        let end_x = this.looper.end().map(to_x);

        let handle = |id: &'static str, x: Option<f32>, ui: &mut nuon::Ui| match x {
            Some(x) => nuon::click_area(id)
                .x(x - GRAB_W / 2.0)
                .size(GRAB_W, h)
                .build(ui),
            None => nuon::ClickAreaEvent::null(),
        };
        let start_ev = handle("LooperStart", start_x, ui);
        let end_ev = handle("LooperEnd", end_x, ui);

        let cursor_time = || {
            let x = ctx.window_state.cursor_logical_position.x;
            this.player.percentage_to_time((x / w).clamp(0.0, 1.0))
        };
        if start_ev.is_pressed() {
            let time = cursor_time();
            this.looper.drag_start(time);
        }
        if end_ev.is_pressed() {
            let time = cursor_time();
            this.looper.drag_end(time);
        }
        if start_ev.is_press_end() || end_ev.is_press_end() {
            this.looper.save(ctx);
        }

        let start_x = this.looper.start().map(to_x);
        let end_x = this.looper.end().map(to_x);
        let active = this.looper.is_active();

        move |ui| {
            if let (Some(start), Some(end)) = (start_x, end_x) {
                let [r, g, b] = LOOP;
                nuon::quad()
                    .x(start)
                    .width(end - start)
                    .height(h)
                    .color([r, g, b, if active { 90 } else { 40 }])
                    .build(ui);
            }

            // The letters sit outside the section, so a short one keeps both readable
            let marks = [(start_x, start_ev, "A", -1.0), (end_x, end_ev, "B", 1.0)];
            for (x, ev, name, side) in marks {
                let Some(x) = x else {
                    continue;
                };
                let hot = ev.is_hovered() || ev.is_pressed();
                let color = if hot { [255; 3] } else { LOOP };
                nuon::quad()
                    .x(x - LINE_W / 2.0)
                    .width(LINE_W)
                    .height(h)
                    .color(color)
                    .build(ui);
                let label_w = 12.0;
                let label_x = if side > 0.0 {
                    x + LINE_W / 2.0 + 1.0
                } else {
                    x - LINE_W / 2.0 - 1.0 - label_w
                };
                nuon::quad()
                    .x(label_x)
                    .width(label_w)
                    .height(13.0)
                    .color(color)
                    .border_radius(if side > 0.0 {
                        [0.0, 3.0, 3.0, 0.0]
                    } else {
                        [3.0, 0.0, 0.0, 3.0]
                    })
                    .build(ui);
                nuon::label()
                    .text(name)
                    .bold(true)
                    .font_size(10.0)
                    .color([20, 18, 24])
                    .x(label_x)
                    .size(label_w, 13.0)
                    .build(ui);
            }
        }
    }
}

/// "70%", "120%"
fn format_speed(speed: f32) -> String {
    format!("{}%", (speed * 100.0).round())
}

/// The next preset above `speed`, wrapping around to the slowest one
fn next_speed_preset(speed: f32) -> f32 {
    SPEED_PRESETS
        .into_iter()
        .find(|&preset| preset > speed + 0.001)
        .unwrap_or(SPEED_PRESETS[0])
}
