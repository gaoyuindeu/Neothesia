//! Grand staff drawn above the waterfall, in sync with playback.
//!
//! Pages turn Synthesia style: the screen is split in two halves of one or two measures
//! (depending on how dense they are). While the left half is playing, the right half shows what comes next;
//! once playback enters the right half, the left half is replaced by the following measures.

use std::{collections::HashMap, time::Duration};

use midi_file::{
    Hand,
    score::{Accidental, Chord, NoteValue, Rest, STAVES, Score, ScoreNote, StaffItem},
};
use wgpu_jumpstart::Color;

use super::{QuadInstance, QuadRenderer, TextRenderer};

const MAX_MEASURES_PER_HALF: usize = 2;
/// A half page holds at most this many chords/rests on one staff, so dense measures get more room
const MAX_ITEMS_PER_HALF: usize = 14;

/// SMuFL code points (Leland font)
mod glyph {
    pub const G_CLEF: char = '\u{E050}';
    pub const F_CLEF: char = '\u{E062}';
    pub const BRACE: char = '\u{E000}';
    pub const NOTEHEAD_WHOLE: char = '\u{E0A2}';
    pub const NOTEHEAD_HALF: char = '\u{E0A3}';
    pub const NOTEHEAD_BLACK: char = '\u{E0A4}';
    pub const SHARP: char = '\u{E262}';
    pub const FLAT: char = '\u{E260}';
    pub const NATURAL: char = '\u{E261}';
    pub const DOT: char = '\u{E1E7}';
    pub const REST_WHOLE: char = '\u{E4E3}';
    pub const REST_HALF: char = '\u{E4E4}';
    pub const REST_QUARTER: char = '\u{E4E5}';
    pub const REST_8TH: char = '\u{E4E6}';
    pub const REST_16TH: char = '\u{E4E7}';
    pub const REST_32ND: char = '\u{E4E8}';
    pub const FLAG_UP: [char; 3] = ['\u{E240}', '\u{E242}', '\u{E244}'];
    pub const FLAG_DOWN: [char; 3] = ['\u{E241}', '\u{E243}', '\u{E245}'];
    pub const OTTAVA_ALTA: char = '\u{E511}';
    pub const OTTAVA_BASSA: char = '\u{E51C}';

    pub fn time_sig_digit(d: u32) -> char {
        char::from_u32(0xE080 + d.min(9)).unwrap()
    }
}

/// Glyph widths in staff spaces (from Leland's metadata, rounded)
const BLACK_HEAD_W: f32 = 1.18;
const WHOLE_HEAD_W: f32 = 1.68;

#[derive(Clone, Copy)]
pub struct SheetColors {
    pub background: [f32; 4],
    pub ink: [u8; 3],
    pub played: [u8; 3],
    /// Highlight of the notes being played: right hand, left hand
    pub hands: [[u8; 3]; 2],
}

pub struct SheetRenderer {
    score: Score,
    /// (first measure, measure count) of every half page
    halves: Vec<(usize, usize)>,
    /// Half page index of every measure
    half_of_measure: Vec<usize>,
    quads: QuadRenderer,
    text: TextRenderer,
    glyphs: GlyphCache,
    colors: SheetColors,
}

/// Where things are, in logical pixels
struct Layout {
    x: f32,
    width: f32,
    /// Staff space
    s: f32,
    /// y of the bottom line of each staff
    bottom: [f32; 2],
    /// Where the measures start (after clefs and signatures)
    content_x: f32,
    content_w: f32,
}

impl Layout {
    /// y of a staff position (0 = bottom line, 8 = top line)
    fn pos_y(&self, staff: usize, pos: i32) -> f32 {
        self.bottom[staff] - pos as f32 * self.s / 2.0
    }

    fn top(&self, staff: usize) -> f32 {
        self.bottom[staff] - 4.0 * self.s
    }
}

impl SheetRenderer {
    pub fn new(score: Score, quads: QuadRenderer, text: TextRenderer, colors: SheetColors) -> Self {
        let items = |m: usize| {
            score.measures[m]
                .staves
                .iter()
                .map(|s| s.len())
                .max()
                .unwrap_or(0)
        };

        let mut halves: Vec<(usize, usize)> = Vec::new();
        let mut half_of_measure = Vec::with_capacity(score.measures.len());
        for m in 0..score.measures.len() {
            let fits = halves.last().is_some_and(|&(first, count)| {
                count < MAX_MEASURES_PER_HALF
                    && (first..first + count).map(items).sum::<usize>() + items(m)
                        <= MAX_ITEMS_PER_HALF
            });
            if fits {
                halves.last_mut().unwrap().1 += 1;
            } else {
                halves.push((m, 1));
            }
            half_of_measure.push(halves.len() - 1);
        }

        Self {
            score,
            halves,
            half_of_measure,
            quads,
            text,
            glyphs: GlyphCache::default(),
            colors,
        }
    }

    pub fn score(&self) -> &Score {
        &self.score
    }

    /// Height the sheet wants for a given staff space
    pub fn height_for(staff_space: f32) -> f32 {
        staff_space * 23.0
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
        let s = height / 23.0;
        let header_w = s * 12.0;
        let layout = Layout {
            x,
            width,
            s,
            bottom: [y + s * 10.0, y + s * 19.0],
            content_x: x + header_w,
            content_w: width - header_w - s * 1.5,
        };

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

        let current = self.score.measure_at(time);
        let half = self.half_of_measure[current];
        // Synthesia style page turn: the half that is not playing shows what comes next
        let (left, right) = if half % 2 == 0 {
            (half, half + 1)
        } else {
            (half + 1, half)
        };

        self.draw_staves(&layout);
        let header_half = self.halves[left.min(right).min(self.halves.len() - 1)];
        self.draw_header(&layout, header_half.0);

        let half_w = layout.content_w / 2.0;
        for (slot, half_index) in [left, right].into_iter().enumerate() {
            let slot_x = layout.content_x + slot as f32 * half_w;
            let Some(&(first, count)) = self.halves.get(half_index) else {
                self.barline(&layout, slot_x + half_w);
                continue;
            };
            let measure_w = half_w / count as f32;

            for i in 0..count {
                let mx = slot_x + i as f32 * measure_w;
                self.draw_measure(&layout, first + i, mx, measure_w, time);
                self.barline(&layout, mx + measure_w);
            }
        }

        self.draw_playhead(&layout, time, left, right);

        self.text.update(physical_size, scale);
        self.quads.prepare();
    }

    pub fn render<'a>(&'a self, rpass: &mut wgpu_jumpstart::RenderPass<'a>) {
        self.quads.render(rpass);
        self.text.render(rpass);
    }

    fn line(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 3]) {
        self.quads.push(QuadInstance {
            position: [x, y],
            size: [w, h],
            color: Color::from_rgba8(color[0], color[1], color[2], 1.0).into_linear_rgba(),
            ..Default::default()
        });
    }

    fn glyph(&mut self, c: char, x: f32, baseline_y: f32, s: f32, color: [u8; 3]) {
        let (buffer, baseline) = self.glyphs.get(c, s * 4.0, color);
        self.text.queue_buffer(x, baseline_y - baseline, buffer);
    }

    fn draw_staves(&mut self, layout: &Layout) {
        let thickness = (layout.s * 0.1).max(1.0);
        let ink = self.colors.ink;
        let x0 = layout.x + layout.s * 1.5;
        let w = layout.x + layout.width - layout.s * 1.5 - x0;
        for staff in 0..2 {
            for line in 0..5 {
                let y = layout.pos_y(staff, line * 2) - thickness / 2.0;
                self.line(x0, y, w, thickness, ink);
            }
        }

        // System barline and brace at the start
        let top = layout.top(0);
        let bottom = layout.bottom[1];
        self.line(x0, top, thickness * 1.5, bottom - top, ink);

        // The brace glyph is one staff (4 spaces) tall at this size; scale it to the system
        let system_h = bottom - top;
        let brace_s = system_h / 4.0;
        let (buffer, baseline) = self.glyphs.get(glyph::BRACE, brace_s * 4.0, ink);
        self.text
            .queue_buffer(x0 - brace_s * 0.45, bottom - baseline, buffer);
    }

    fn draw_header(&mut self, layout: &Layout, first_measure: usize) {
        let s = layout.s;
        let ink = self.colors.ink;
        let measure = &self.score.measures[first_measure.min(self.score.measures.len() - 1)];
        let key = measure.key;
        let time_sig = measure.time_signature;

        let clef_x = layout.x + s * 2.5;
        // G clef curls around G4 (2nd line), F clef around F3 (4th line)
        self.glyph(glyph::G_CLEF, clef_x, layout.pos_y(0, 2), s, ink);
        self.glyph(glyph::F_CLEF, clef_x, layout.pos_y(1, 6), s, ink);

        // Key signature, positions relative to the bottom line of the treble staff
        const SHARPS: [i32; 7] = [8, 5, 9, 6, 3, 7, 4];
        const FLATS: [i32; 7] = [4, 7, 3, 6, 2, 5, 1];
        let mut x = clef_x + s * 3.2;
        let (positions, symbol) = if key >= 0 {
            (&SHARPS[..key as usize], glyph::SHARP)
        } else {
            (&FLATS[..(-key) as usize], glyph::FLAT)
        };
        for &pos in positions {
            self.glyph(symbol, x, layout.pos_y(0, pos), s, ink);
            // Bass staff: the same pattern two positions lower
            self.glyph(symbol, x, layout.pos_y(1, pos - 2), s, ink);
            x += s;
        }

        // Time signature
        let x = x + s * 0.6;
        for staff in 0..2 {
            for (digits, pos) in [(time_sig.0 as u32, 6), (time_sig.1 as u32, 2)] {
                let text: Vec<u32> = digits
                    .to_string()
                    .chars()
                    .filter_map(|c| c.to_digit(10))
                    .collect();
                for (i, d) in text.into_iter().enumerate() {
                    self.glyph(
                        glyph::time_sig_digit(d),
                        x + i as f32 * s * 1.8,
                        layout.pos_y(staff, pos),
                        s,
                        ink,
                    );
                }
            }
        }
    }

    fn barline(&mut self, layout: &Layout, x: f32) {
        let thickness = (layout.s * 0.12).max(1.0);
        let top = layout.top(0);
        let bottom = layout.bottom[1];
        self.line(
            x - thickness / 2.0,
            top,
            thickness,
            bottom - top,
            self.colors.ink,
        );
    }

    fn tick_x(&self, index: usize, tick: f64, mx: f32, mw: f32, s: f32) -> f32 {
        let measure = &self.score.measures[index];
        let pad = s * 1.8;
        let t = (tick - measure.start_tick as f64) / (measure.end_tick - measure.start_tick) as f64;
        mx + pad + t as f32 * (mw - pad * 2.0)
    }

    fn draw_measure(&mut self, layout: &Layout, index: usize, mx: f32, mw: f32, time: Duration) {
        let s = layout.s;
        for staff in 0..2 {
            let items = self.score.measures[index].staves[staff].clone();
            for item in items {
                match item {
                    StaffItem::Chord(chord) => {
                        let x = self.tick_x(index, chord.tick as f64, mx, mw, s);
                        self.draw_chord(layout, staff, &chord, x, time);
                    }
                    StaffItem::Rest(rest) => {
                        let x = if rest.whole_measure {
                            mx + mw / 2.0 - s * 0.7
                        } else {
                            self.tick_x(index, rest.tick as f64, mx, mw, s)
                        };
                        self.draw_rest(layout, staff, &rest, x);
                    }
                }
            }
        }
    }

    fn note_color(&self, staff: usize, note: &ScoreNote, time: Duration) -> [u8; 3] {
        let hand = match note.hand {
            Some(Hand::Right) => 0,
            Some(Hand::Left) => 1,
            None => staff,
        };
        let (start, end) = (note.start, note.end);
        if start <= time && time < end {
            self.colors.hands[hand]
        } else if end <= time {
            self.colors.played
        } else {
            self.colors.ink
        }
    }

    fn draw_chord(&mut self, layout: &Layout, staff: usize, chord: &Chord, x: f32, time: Duration) {
        let s = layout.s;
        let kind = STAVES[staff];
        let bottom_step = kind.bottom_line_step();
        let head = match chord.value {
            NoteValue::Whole => glyph::NOTEHEAD_WHOLE,
            NoteValue::Half => glyph::NOTEHEAD_HALF,
            _ => glyph::NOTEHEAD_BLACK,
        };
        let head_w = if chord.value == NoteValue::Whole {
            WHOLE_HEAD_W
        } else {
            BLACK_HEAD_W
        } * s;

        let mut positions: Vec<i32> = chord.notes.iter().map(|n| n.step - bottom_step).collect();

        // Chords far outside the staff are written an octave closer, marked 8va / 8vb
        let ottava = if staff == 0 && *positions.iter().min().unwrap() > 13 {
            Some((-7, glyph::OTTAVA_ALTA))
        } else if staff == 1 && *positions.iter().max().unwrap() < -5 {
            Some((7, glyph::OTTAVA_BASSA))
        } else {
            None
        };
        if let Some((shift, _)) = ottava {
            positions.iter_mut().for_each(|p| *p += shift);
        }
        let lowest = *positions.iter().min().unwrap();
        let highest = *positions.iter().max().unwrap();
        // Stem goes down when the notes sit mostly above the middle line
        let stem_up = (lowest + highest) < 8;

        // Heads a second apart can't share a column: move every other one across the stem
        let mut offsets = vec![0.0; positions.len()];
        for i in 1..positions.len() {
            if positions[i] - positions[i - 1] == 1 && offsets[i - 1] == 0.0 {
                offsets[i] = if stem_up { head_w } else { -head_w };
            }
        }

        let ink = self.colors.ink;
        let ledger_w = head_w * 1.5;
        let thickness = (s * 0.12).max(1.0);

        // Ledger lines
        for pos in (10..=highest).step_by(2) {
            let y = layout.pos_y(staff, pos) - thickness / 2.0;
            self.line(x - (ledger_w - head_w) / 2.0, y, ledger_w, thickness, ink);
        }
        let mut pos = -2;
        while pos >= lowest {
            let y = layout.pos_y(staff, pos) - thickness / 2.0;
            self.line(x - (ledger_w - head_w) / 2.0, y, ledger_w, thickness, ink);
            pos -= 2;
        }

        let chord_color = chord
            .notes
            .iter()
            .map(|n| self.note_color(staff, n, time))
            .find(|c| *c != self.colors.ink && *c != self.colors.played)
            .unwrap_or_else(|| self.note_color(staff, &chord.notes[0], time));

        if let Some((_, mark)) = ottava {
            // Just outside the notes (and their stems)
            let y = if staff == 0 {
                layout.pos_y(staff, highest.max(8) + 2)
            } else {
                layout.pos_y(staff, lowest.min(0) - 5)
            };
            self.glyph(mark, x - s * 0.2, y, s * 0.6, chord_color);
        }

        for (i, note) in chord.notes.iter().enumerate() {
            let color = self.note_color(staff, note, time);
            let hx = x + offsets[i];
            let y = layout.pos_y(staff, positions[i]);
            self.glyph(head, hx, y, s, color);

            if let Some(accidental) = note.accidental {
                let c = match accidental {
                    Accidental::Sharp => glyph::SHARP,
                    Accidental::Flat => glyph::FLAT,
                    Accidental::Natural => glyph::NATURAL,
                };
                self.glyph(c, x - s * 1.3, y, s, color);
            }

            if chord.dotted {
                // Dots sit in a space, never on a line
                let dot_pos = if positions[i] % 2 == 0 {
                    positions[i] + 1
                } else {
                    positions[i]
                };
                let extra = offsets.iter().cloned().fold(0.0, f32::max);
                self.glyph(
                    glyph::DOT,
                    x + head_w + extra + s * 0.3,
                    layout.pos_y(staff, dot_pos),
                    s,
                    color,
                );
            }
        }

        if !chord.value.has_stem() {
            return;
        }

        let stem_w = (s * 0.12).max(1.0);
        let length = s * 3.5;
        let (stem_x, y_from, y_to) = if stem_up {
            let from = layout.pos_y(staff, lowest) - s * 0.17;
            let to = layout.pos_y(staff, highest) - length;
            (x + head_w - stem_w, from, to)
        } else {
            let from = layout.pos_y(staff, highest) + s * 0.17;
            let to = layout.pos_y(staff, lowest) + length;
            (x, from, to)
        };
        let (top, bottom) = if y_from < y_to {
            (y_from, y_to)
        } else {
            (y_to, y_from)
        };
        self.line(stem_x, top, stem_w, bottom - top, chord_color);

        let flags = chord.value.flags();
        if flags > 0 {
            let flag = if stem_up {
                glyph::FLAG_UP[flags as usize - 1]
            } else {
                glyph::FLAG_DOWN[flags as usize - 1]
            };
            self.glyph(flag, stem_x, y_to, s, chord_color);
        }
    }

    fn draw_rest(&mut self, layout: &Layout, staff: usize, rest: &Rest, x: f32) {
        let s = layout.s;
        let (c, pos) = match rest.value {
            // The whole rest hangs from the 4th line, the half rest sits on the middle one
            NoteValue::Whole => (glyph::REST_WHOLE, 6),
            NoteValue::Half => (glyph::REST_HALF, 4),
            NoteValue::Quarter => (glyph::REST_QUARTER, 4),
            NoteValue::Eighth => (glyph::REST_8TH, 4),
            NoteValue::Sixteenth => (glyph::REST_16TH, 4),
            NoteValue::ThirtySecond => (glyph::REST_32ND, 4),
        };
        let color = self.colors.played;
        self.glyph(c, x, layout.pos_y(staff, pos), s, color);
        if rest.dotted {
            self.glyph(glyph::DOT, x + s * 1.5, layout.pos_y(staff, 5), s, color);
        }
    }

    fn draw_playhead(&mut self, layout: &Layout, time: Duration, left: usize, right: usize) {
        let index = self.score.measure_at(time);
        let half = self.half_of_measure[index];
        let slot = if half == left {
            0
        } else if half == right {
            1
        } else {
            return;
        };

        let measure = &self.score.measures[index];
        if time < measure.start || time > measure.end {
            return;
        }

        let (first, count) = self.halves[half];
        let half_w = layout.content_w / 2.0;
        let measure_w = half_w / count as f32;
        let mx = layout.content_x + slot as f32 * half_w + (index - first) as f32 * measure_w;
        let x = self.tick_x(
            index,
            self.score.time_to_tick(time),
            mx,
            measure_w,
            layout.s,
        );

        let top = layout.top(0) - layout.s * 2.0;
        let bottom = layout.bottom[1] + layout.s * 2.0;
        let [r, g, b] = self.colors.hands[0];
        self.quads.push(QuadInstance {
            position: [x - layout.s * 0.15, top],
            size: [layout.s * 0.3, bottom - top],
            color: Color::from_rgba8(r, g, b, 0.45).into_linear_rgba(),
            border_radius: [layout.s * 0.15; 4],
            ..Default::default()
        });
    }
}

/// Shaped single glyph buffers, reused between frames
#[derive(Default)]
struct GlyphCache {
    buffers: HashMap<(char, [u8; 3], u32), (glyphon::Buffer, f32)>,
}

impl GlyphCache {
    /// Returns the buffer and the distance from its top to the glyph baseline
    fn get(&mut self, c: char, size: f32, color: [u8; 3]) -> (glyphon::Buffer, f32) {
        let key = (c, color, size.to_bits());
        if let Some(entry) = self.buffers.get(&key) {
            return entry.clone();
        }
        // Sizes change on resize; don't keep old ones around forever
        if self.buffers.len() > 512 {
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
