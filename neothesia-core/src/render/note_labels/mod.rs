use crate::utils::Point;

use super::{KeyboardRenderer, TextRenderer, waterfall::NoteList};

fn label_buffer(
    font_system: &mut glyphon::FontSystem,
    text: &str,
    font_size: f32,
    width: f32,
    bold: bool,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(font_system, glyphon::Metrics::new(font_size, font_size));
    buffer.set_size(Some(width), None);
    buffer.set_wrap(glyphon::Wrap::None);
    let mut attrs = glyphon::Attrs::new().family(glyphon::Family::SansSerif);
    if bold {
        attrs = attrs.weight(glyphon::Weight::BOLD);
    }
    buffer.set_text(
        text,
        &attrs,
        glyphon::Shaping::Basic,
        Some(glyphon::cosmic_text::Align::Center),
    );
    buffer.shape_until_scroll(font_system, false);
    buffer
}

#[derive(Default)]
struct LabelsCache {
    labels: Option<[glyphon::Buffer; 12]>,
    /// Finger numbers 1..=5, for white and black keys
    fingers: Option<[[glyphon::Buffer; 5]; 2]>,
    neutral_width: f32,
}

impl LabelsCache {
    #[profiling::function]
    fn update(&mut self, keyboard: &KeyboardRenderer) {
        let sharp_width = keyboard.layout().sizing.sharp_width;
        let neutral_width = keyboard.layout().sizing.neutral_width;

        if self.labels.is_some() && self.neutral_width == neutral_width {
            return;
        }

        let font_system = crate::font_system::font_system();
        let font_system = &mut font_system.borrow_mut();

        let label_width = sharp_width;
        let labels = [
            ("C", neutral_width),
            ("C#", sharp_width),
            ("D", neutral_width),
            ("D#", sharp_width),
            ("E", neutral_width),
            ("F", neutral_width),
            ("F#", sharp_width),
            ("G", neutral_width),
            ("G#", sharp_width),
            ("A", neutral_width),
            ("A#", sharp_width),
            ("B", neutral_width),
        ]
        .map(|(label, note_width)| {
            label_buffer(font_system, label, label_width, note_width, false)
        });

        let fingers = [neutral_width, sharp_width].map(|note_width| {
            [1, 2, 3, 4, 5].map(|d| {
                label_buffer(
                    font_system,
                    &d.to_string(),
                    label_width * 1.1,
                    note_width,
                    true,
                )
            })
        });

        self.labels = Some(labels);
        self.fingers = Some(fingers);
        self.neutral_width = neutral_width;
    }
}

/// What to write on the falling notes
#[derive(Debug, Clone, Copy, Default)]
pub struct LabelOptions {
    /// Note names (C, C#, ...)
    pub names: bool,
    /// Finger numbers from the score
    pub printed_fingers: bool,
    /// Estimated finger numbers
    pub estimated_fingers: bool,
}

impl LabelOptions {
    pub fn any(&self) -> bool {
        self.names || self.printed_fingers || self.estimated_fingers
    }
}

pub struct NoteLabels {
    pos: Point<f32>,
    notes: NoteList,
    labels_cache: LabelsCache,
    text_renderer: TextRenderer,
    options: LabelOptions,
}

impl NoteLabels {
    pub fn new(
        pos: Point<f32>,
        notes: &NoteList,
        text_renderer: TextRenderer,
        options: LabelOptions,
    ) -> Self {
        Self {
            pos,
            notes: notes.clone(),
            labels_cache: LabelsCache::default(),
            text_renderer,
            options,
        }
    }

    pub fn set_pos(&mut self, pos: Point<f32>) {
        self.pos = pos;
    }

    #[profiling::function]
    pub fn update(
        &mut self,
        physical_size: dpi::PhysicalSize<u32>,
        scale: f32,
        keyboard: &KeyboardRenderer,
        animation_speed: f32,
        time: f32,
    ) {
        let layout = keyboard.layout();
        let range_start = layout.range.start() as usize;
        let label_width = layout.sizing.sharp_width;

        self.labels_cache.update(keyboard);
        let labels = self.labels_cache.labels.as_ref().unwrap();
        let fingers = self.labels_cache.fingers.as_ref().unwrap();
        let options = self.options;
        let animation_speed = animation_speed / scale;

        let iter = self
            .notes
            .inner
            .iter()
            .filter(|note| layout.range.contains(note.note) && note.channel != 9)
            .map(|note| {
                let key = &layout.keys[note.note as usize - range_start];
                let y =
                    self.pos.y - (note.start.as_secs_f32() - time) * animation_speed - label_width;
                (note, key, y)
            })
            // Stop iteration once we reach top of the screen
            .take_while(|(_note, _key, y)| *y > 0.0)
            // TODO: Cache last note idx to skip this NoOp skip iteration
            .skip_while(|(_note, _key, y)| *y > keyboard.pos().y)
            .filter_map(|(note, key, top)| {
                let finger = note.finger.filter(|f| {
                    (1..=5).contains(&f.digit)
                        && if f.printed {
                            options.printed_fingers
                        } else {
                            options.estimated_fingers
                        }
                });
                let (buffer, color) = if let Some(f) = finger {
                    let black = key.kind().is_sharp() as usize;
                    // Estimated fingers are a little fainter than printed ones
                    let alpha = if f.printed { 255 } else { 190 };
                    (
                        &fingers[black][f.digit as usize - 1],
                        glyphon::Color::rgba(255, 255, 255, alpha),
                    )
                } else if options.names {
                    (
                        &labels[(note.note % 12) as usize],
                        glyphon::Color::rgb(255, 255, 255),
                    )
                } else {
                    return None;
                };
                Some(glyphon::TextArea {
                    buffer,
                    left: key.x(),
                    top,
                    scale: 1.0,
                    bounds: glyphon::TextBounds {
                        left: 0,
                        top: 0,
                        right: i32::MAX,
                        bottom: i32::MAX,
                    },
                    default_color: color,
                    custom_glyphs: &[],
                })
            });

        self.text_renderer
            .update_from_iter(physical_size, scale, iter);
    }

    pub fn render<'rpass>(&'rpass mut self, render_pass: &mut wgpu_jumpstart::RenderPass<'rpass>) {
        self.text_renderer.render(render_pass);
    }
}
