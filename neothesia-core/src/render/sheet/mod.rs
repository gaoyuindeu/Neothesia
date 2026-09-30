//! Grand staff drawn above the waterfall, in sync with playback.
//!
//! Pages turn Synthesia style: the sheet is split in two halves, each holding as many
//! measures as fit. While the left half is playing, the right half shows what comes next;
//! once playback enters the right half, the left half is replaced by the measures after it.

mod engrave;
mod glyphs;

use std::{collections::HashMap, time::Duration};

use midi_file::score::Score;
use wgpu_jumpstart::Color;

use self::{
    engrave::{
        Element, Engraved, Ink, MeasurePlan, Options, PANEL_HEIGHT, STAFF_BOTTOM, Signature,
    },
    glyphs::Metrics,
};
use super::{QuadInstance, QuadRenderer, TextRenderer};

const MAX_MEASURES_PER_HALF: usize = 4;

#[derive(Clone, Copy)]
pub struct SheetColors {
    pub background: [f32; 4],
    pub ink: [u8; 3],
    /// Notes that were already played, rests and secondary marks
    pub played: [u8; 3],
    /// Highlight of the notes being played: right hand, left hand
    pub hands: [[u8; 3]; 2],
}

/// Which measures make up each half page, for a given half width
struct Pages {
    half_width: f32,
    /// (first measure, count)
    halves: Vec<(usize, usize)>,
    half_of_measure: Vec<usize>,
}

pub struct SheetRenderer {
    score: Score,
    quads: QuadRenderer,
    text: TextRenderer,
    glyphs: GlyphCache,
    colors: SheetColors,
    metrics: Metrics,
    /// Natural spacing of every measure, with its key/time changes drawn inline
    plans: Vec<MeasurePlan>,
    header_width: f32,
    pages: Option<Pages>,
    /// Engraved measures by (index, width, options)
    cache: HashMap<(usize, u32, bool, Signature), Engraved>,
}

impl SheetRenderer {
    pub fn new(score: Score, quads: QuadRenderer, text: TextRenderer, colors: SheetColors) -> Self {
        let mut glyphs = GlyphCache::default();
        let metrics = measure_metrics(&mut glyphs);

        let plans = (0..score.measures.len())
            .map(|i| {
                let signature = Signature::changes(&score.measures[i]);
                engrave::plan(&score, i, &metrics, signature)
            })
            .collect();

        // Room for the clefs, the widest key signature and a time signature
        let widest_key = score
            .measures
            .iter()
            .map(|m| engrave::key_signature_width(m.key, None, &metrics))
            .fold(0.0, f32::max);
        let widest_time = score
            .measures
            .iter()
            .map(|m| engrave::time_signature_width(m.time_signature, &metrics))
            .fold(0.0, f32::max);
        let header_width = 1.5 + metrics.clef + 1.0 + widest_key + widest_time + 0.5;

        Self {
            score,
            quads,
            text,
            glyphs,
            colors,
            metrics,
            plans,
            header_width,
            pages: None,
            cache: HashMap::new(),
        }
    }

    pub fn score(&self) -> &Score {
        &self.score
    }

    /// Height of the sheet for a given staff space
    pub fn height_for(staff_space: f32) -> f32 {
        staff_space * PANEL_HEIGHT
    }

    fn natural_width(&self, index: usize) -> f32 {
        self.plans[index].natural_width
    }

    fn paginate(&mut self, half_width: f32) {
        if self
            .pages
            .as_ref()
            .is_some_and(|p| (p.half_width - half_width).abs() < 0.5)
        {
            return;
        }

        let mut halves: Vec<(usize, usize)> = Vec::new();
        let mut half_of_measure = Vec::with_capacity(self.score.measures.len());
        let mut used = 0.0;
        for m in 0..self.score.measures.len() {
            let w = self.natural_width(m);
            let fits = halves
                .last()
                .is_some_and(|&(_, count)| count < MAX_MEASURES_PER_HALF && used + w <= half_width);
            if fits {
                halves.last_mut().unwrap().1 += 1;
                used += w;
            } else {
                halves.push((m, 1));
                used = w;
            }
            half_of_measure.push(halves.len() - 1);
        }

        self.pages = Some(Pages {
            half_width,
            halves,
            half_of_measure,
        });
        self.cache.clear();
    }

    /// Lay out and queue everything for this frame.
    ///
    /// `time` is the song time (without lead-in); `rect` is (x, y, width, height).
    pub fn update(
        &mut self,
        time: Duration,
        rect: (f32, f32, f32, f32),
        physical_size: dpi::PhysicalSize<u32>,
        scale: f32,
    ) {
        self.quads.clear();

        let (x, y, width, height) = rect;
        let s = height / PANEL_HEIGHT;

        self.quads.push(QuadInstance {
            position: [x, y],
            size: [width, height],
            color: self.colors.background,
            border_radius: [0.0, 0.0, s, s],
            ..Default::default()
        });

        if self.score.measures.is_empty() {
            self.text.update(physical_size, scale);
            self.quads.prepare();
            return;
        }

        let total = width / s;
        let right_margin = 1.5;
        let half_width = ((total - self.header_width - right_margin) / 2.0).max(4.0);
        self.paginate(half_width);
        let pages = self.pages.as_ref().unwrap();

        let current = self.score.measure_at(time);
        let half = pages.half_of_measure[current];
        // The half that is not playing shows what comes next
        let (left, right) = if half % 2 == 0 {
            (half, half + 1)
        } else {
            (half + 1, half)
        };

        let mut canvas = Canvas {
            quads: &mut self.quads,
            text: &mut self.text,
            glyphs: &mut self.glyphs,
            colors: self.colors,
            time,
            origin: (x, y),
            s,
            pixel: 1.0 / scale.max(0.1),
        };

        staves(&mut canvas, total, right_margin);

        // Header: clefs, key and (at the start or on a change) time signature
        let header_measure = pages
            .halves
            .get(left)
            .map_or(self.score.measures.len() - 1, |h| h.0);
        let mut header = Vec::new();
        let clef_x = 1.8;
        let measure = &self.score.measures[header_measure];
        let header_clefs = [measure.staves[0].clef, measure.staves[1].clef];
        for (staff, clef) in header_clefs.iter().enumerate() {
            let (c, line) = glyphs::clef(*clef);
            header.push(Element::Glyph {
                c,
                x: clef_x,
                y: engrave::pos_y(staff, line),
                size: 1.0,
                ink: Ink::Plain,
            });
        }
        let (header_key, header_time) = (measure.key, measure.time_signature);
        let mut hx = clef_x + self.metrics.clef + 0.8;
        hx += engrave::key_signature(
            &mut header,
            hx,
            measure.key,
            None,
            header_clefs,
            &self.metrics,
        );
        if measure.index == 0 || measure.time_signature_changed {
            engrave::time_signature(&mut header, hx, measure.time_signature, &self.metrics);
        }
        for element in &header {
            canvas.draw(element, 0.0);
        }

        let mut playhead = None;
        for (slot, half_index) in [left, right].into_iter().enumerate() {
            let slot_x = self.header_width + slot as f32 * half_width;
            let Some(&(first, count)) = pages.halves.get(half_index) else {
                continue;
            };
            let natural: f32 = (first..first + count)
                .map(|m| self.plans[m].natural_width)
                .sum();
            let factor = half_width / natural.max(0.1);

            let mut mx = slot_x;
            for i in 0..count {
                let index = first + i;
                let measure = &self.score.measures[index];
                let signature = match (slot, i) {
                    // The header shows the key/time of the left half's first measure
                    (0, 0) => Signature::default(),
                    // The right half may be in another key than the header says
                    (_, 0) => Signature {
                        key: (measure.key != header_key).then_some((measure.key, Some(header_key))),
                        time: (measure.time_signature != header_time
                            || measure.time_signature_changed)
                            .then_some(measure.time_signature),
                        clefs: [0, 1].map(|s| {
                            let clef = measure.staves[s].clef;
                            (clef != header_clefs[s]).then_some(clef)
                        }),
                    },
                    _ => Signature::changes(measure),
                };
                let options = Options {
                    signature,
                    first_in_half: i == 0,
                    last_measure: index + 1 == self.score.measures.len(),
                };
                let w = self.plans[index].natural_width * factor;
                let key = (index, (w * 100.0).round() as u32, i == 0, signature);
                let engraved = self.cache.entry(key).or_insert_with(|| {
                    let plan = engrave::plan(&self.score, index, &self.metrics, signature);
                    engrave::engrave(&self.score, index, &plan, w, &self.metrics, options)
                });

                for element in &engraved.elements {
                    canvas.draw(element, mx);
                }

                if index == current {
                    let tick = self.score.time_to_tick(time);
                    playhead = Some(mx + engrave::x_at(engraved, tick));
                }
                mx += w;
            }
        }

        if let Some(px) = playhead {
            let top = engrave::staff_top(0) - 2.0;
            let bottom = STAFF_BOTTOM[1] + 2.0;
            let [r, g, b] = self.colors.hands[0];
            canvas.quads.push(QuadInstance {
                position: [x + (px - 0.15) * s, y + top * s],
                size: [0.3 * s, (bottom - top) * s],
                color: Color::from_rgba8(r, g, b, 0.4).into_linear_rgba(),
                border_radius: [0.15 * s; 4],
                ..Default::default()
            });
        }

        self.text.update(physical_size, scale);
        self.quads.prepare();
    }

    pub fn render<'a>(&'a self, rpass: &mut wgpu_jumpstart::RenderPass<'a>) {
        self.quads.render(rpass);
        self.text.render(rpass);
    }
}

fn staves(canvas: &mut Canvas, total_width: f32, right_margin: f32) {
    let x0 = 1.2;
    let w = total_width - right_margin - x0;
    for staff in 0..2 {
        for line in 0..5 {
            let y = engrave::pos_y(staff, line * 2);
            canvas.draw(
                &Element::Rect {
                    x: x0,
                    y: y - 0.065,
                    w,
                    h: 0.13,
                    ink: Ink::Plain,
                },
                0.0,
            );
        }
    }

    let top = engrave::staff_top(0);
    let bottom = STAFF_BOTTOM[1];
    canvas.draw(
        &Element::Rect {
            x: x0,
            y: top,
            w: 0.16,
            h: bottom - top,
            ink: Ink::Plain,
        },
        0.0,
    );

    // The brace glyph is one staff (4 spaces) tall at size 1; scale it to the system
    let size = (bottom - top) / 4.0;
    canvas.draw(
        &Element::Glyph {
            c: glyphs::BRACE,
            x: x0 - 0.45 * size,
            y: bottom,
            size,
            ink: Ink::Plain,
        },
        0.0,
    );
}

/// Turns elements (in staff spaces) into quads and glyphs (in logical pixels)
struct Canvas<'a> {
    quads: &'a mut QuadRenderer,
    text: &'a mut TextRenderer,
    glyphs: &'a mut GlyphCache,
    colors: SheetColors,
    time: Duration,
    origin: (f32, f32),
    /// Staff space in logical pixels
    s: f32,
    /// One physical pixel in logical pixels
    pixel: f32,
}

impl Canvas<'_> {
    fn color(&self, ink: Ink) -> [u8; 3] {
        match ink {
            Ink::Plain => self.colors.ink,
            Ink::Dim => self.colors.played,
            Ink::Note(note) => {
                if note.start <= self.time && self.time < note.end {
                    self.colors.hands[note.hand.min(1)]
                } else if note.end <= self.time {
                    self.colors.played
                } else {
                    self.colors.ink
                }
            }
            Ink::Estimated(note) => {
                if note.end <= self.time {
                    self.colors.played
                } else {
                    self.colors.hands[note.hand.min(1)]
                }
            }
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 3]) {
        self.quads.push(QuadInstance {
            position: [x, y],
            size: [w, h],
            color: Color::from_rgba8(color[0], color[1], color[2], 1.0).into_linear_rgba(),
            ..Default::default()
        });
    }

    fn draw(&mut self, element: &Element, dx: f32) {
        let (ox, oy) = self.origin;
        let s = self.s;
        let px = |x: f32| ox + (x + dx) * s;
        let py = |y: f32| oy + y * s;

        match *element {
            Element::Glyph { c, x, y, size, ink } => {
                let color = self.color(ink);
                let (buffer, baseline) = self.glyphs.get(c, size * 4.0 * s, color);
                self.text.queue_buffer(px(x), py(y) - baseline, buffer);
            }
            Element::Rect { x, y, w, h, ink } => {
                let color = self.color(ink);
                // Keep hairlines visible at small sizes
                let w = (w * s).max(self.pixel);
                let h = (h * s).max(self.pixel);
                self.rect(px(x), py(y), w, h, color);
            }
            Element::Beam {
                x0,
                y0,
                x1,
                y1,
                thickness,
                ink,
            } => {
                let color = self.color(ink);
                let (a, b) = (px(x0), px(x1));
                let step = self.pixel;
                let slope = (y1 - y0) / (x1 - x0).max(0.001);
                let mut x = a;
                while x < b {
                    let w = step.min(b - x);
                    let t = (x - a + w / 2.0) / s;
                    let top = py(y0 + slope * t);
                    self.rect(x, top, w + step * 0.3, thickness * s, color);
                    x += step;
                }
            }
            Element::Tie {
                x0,
                x1,
                y,
                height,
                ink,
            } => {
                let color = self.color(ink);
                let (a, b) = (px(x0), px(x1));
                let step = self.pixel;
                let mut x = a;
                while x < b {
                    let u = (x - a + step / 2.0) / (b - a);
                    let center = y + height * 4.0 * u * (1.0 - u);
                    let thickness = 0.05 + 0.16 * (std::f32::consts::PI * u).sin();
                    self.rect(
                        x,
                        py(center - thickness / 2.0),
                        step + step * 0.3,
                        (thickness * s).max(self.pixel),
                        color,
                    );
                    x += step;
                }
            }
            Element::Dashes { x0, x1, y, ink } => {
                let color = self.color(ink);
                let mut x = x0;
                while x < x1 {
                    let w = 0.6f32.min(x1 - x);
                    self.rect(px(x), py(y - 0.05), w * s, (0.1 * s).max(self.pixel), color);
                    x += 1.0;
                }
            }
            Element::Wiggle { x, y0, y1, ink } => {
                let color = self.color(ink);
                let (top, bottom) = (py(y0), py(y1));
                let step = self.pixel;
                let mut y = top;
                while y < bottom {
                    let phase = (y - top) / (0.9 * s) * std::f32::consts::TAU;
                    let offset = 0.22 * s * phase.sin();
                    self.rect(
                        px(x) + offset,
                        y,
                        (0.16 * s).max(self.pixel),
                        step * 1.3,
                        color,
                    );
                    y += step;
                }
            }
            Element::Slur {
                x0,
                y0,
                x1,
                y1,
                height,
                ink,
            } => {
                let color = self.color(ink);
                let (a, b) = (px(x0), px(x1));
                let step = self.pixel;
                let mut x = a;
                while x < b {
                    let u = (x - a + step / 2.0) / (b - a);
                    let center = y0 + (y1 - y0) * u + height * 4.0 * u * (1.0 - u);
                    let thickness = 0.05 + 0.13 * (std::f32::consts::PI * u).sin();
                    self.rect(
                        x,
                        py(center - thickness / 2.0),
                        step * 1.3,
                        (thickness * s).max(self.pixel),
                        color,
                    );
                    x += step;
                }
            }
            Element::Text {
                ref text,
                x,
                y,
                size,
                italic: _,
                bold,
                ink,
            } => {
                let color = self.color(ink);
                let (buffer, baseline) = self.glyphs.text(text, size * s, bold, color);
                self.text.queue_buffer(px(x), py(y) - baseline, buffer);
            }
        }
    }
}

fn measure_metrics(glyphs: &mut GlyphCache) -> Metrics {
    // Advance widths at a large size, in staff spaces (1 em = 4 spaces)
    let size = 400.0;
    let mut width = |c: char, fallback: f32| {
        let (buffer, _) = glyphs.get(c, size, [255, 255, 255]);
        buffer
            .layout_runs()
            .next()
            .map(|run| run.line_w / size * 4.0)
            .filter(|w| *w > 0.05)
            .unwrap_or(fallback)
    };
    let d = Metrics::default();
    let metrics = Metrics {
        head_black: width(glyphs::NOTEHEAD_BLACK, d.head_black),
        head_half: width(glyphs::NOTEHEAD_HALF, d.head_half),
        head_whole: width(glyphs::NOTEHEAD_WHOLE, d.head_whole),
        sharp: width(glyphs::SHARP, d.sharp),
        flat: width(glyphs::FLAT, d.flat),
        natural: width(glyphs::NATURAL, d.natural),
        dot: width(glyphs::DOT, d.dot),
        flag: width(glyphs::FLAG_UP[0], d.flag),
        time_digit: width(glyphs::time_sig_digit(4), d.time_digit),
        clef: width(glyphs::G_CLEF, d.clef),
    };
    glyphs.buffers.clear();
    metrics
}

/// Shaped single glyph buffers, reused between frames
#[derive(Default)]
struct GlyphCache {
    buffers: HashMap<(char, [u8; 3], u32), (glyphon::Buffer, f32)>,
    texts: HashMap<(String, u32, bool, [u8; 3]), (glyphon::Buffer, f32)>,
}

impl GlyphCache {
    /// Text in the regular UI font; returns the buffer and its baseline offset
    fn text(
        &mut self,
        text: &str,
        size: f32,
        bold: bool,
        color: [u8; 3],
    ) -> (glyphon::Buffer, f32) {
        let key = (text.to_string(), size.to_bits(), bold, color);
        if let Some(entry) = self.texts.get(&key) {
            return entry.clone();
        }
        if self.texts.len() > 512 {
            self.texts.clear();
        }
        let mut attrs = glyphon::Attrs::new()
            .family(glyphon::Family::Name("Roboto"))
            .color(glyphon::Color::rgb(color[0], color[1], color[2]));
        if bold {
            attrs = attrs.weight(glyphon::Weight::BOLD);
        }
        let buffer = TextRenderer::gen_buffer_with_attr(size, text, attrs);
        let baseline = buffer
            .layout_runs()
            .next()
            .map(|run| run.line_y)
            .unwrap_or(size);
        self.texts.insert(key, (buffer.clone(), baseline));
        (buffer, baseline)
    }

    /// Returns the buffer and the distance from its top to the glyph baseline
    fn get(&mut self, c: char, size: f32, color: [u8; 3]) -> (glyphon::Buffer, f32) {
        let key = (c, color, size.to_bits());
        if let Some(entry) = self.buffers.get(&key) {
            return entry.clone();
        }
        // Sizes change on resize; don't keep old ones around forever
        if self.buffers.len() > 1024 {
            self.buffers.clear();
        }
        let buffer = TextRenderer::gen_buffer_with_attr(
            size,
            &c.to_string(),
            glyphon::Attrs::new()
                .family(glyphon::Family::Name("Leland"))
                .color(glyphon::Color::rgb(color[0], color[1], color[2])),
        );
        let baseline = buffer
            .layout_runs()
            .next()
            .map(|run| run.line_y)
            .unwrap_or(size);
        self.buffers.insert(key, (buffer.clone(), baseline));
        (buffer, baseline)
    }
}
