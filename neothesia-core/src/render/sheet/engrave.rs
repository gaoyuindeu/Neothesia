//! Layout of one measure of the grand staff.
//!
//! Everything is measured in staff spaces: x from the left edge of the measure,
//! y downwards from the top of the sheet panel. The renderer scales it to pixels.

use std::{collections::HashMap, time::Duration};

use midi_file::{
    Hand,
    score::{Accidental, Articulation, Clef, Event, Measure, NoteValue, Ornament, Score, Voice},
};

use super::glyphs::{self, Metrics};

/// Height of the sheet panel
pub const PANEL_HEIGHT: f32 = 25.0;
/// y of the bottom line of the treble and the bass staff
pub const STAFF_BOTTOM: [f32; 2] = [10.5, 19.5];

const STEM_WIDTH: f32 = 0.12;
const STEM_LENGTH: f32 = 3.5;
const LEDGER_WIDTH: f32 = 0.16;
const LEDGER_EXTENSION: f32 = 0.4;
const BEAM_THICKNESS: f32 = 0.5;
const BEAM_DISTANCE: f32 = 0.75;
const BARLINE_WIDTH: f32 = 0.16;
/// Grace notes are drawn at this size, this far apart
const GRACE_SCALE: f32 = 0.65;
const GRACE_SPACING: f32 = 1.6;
/// Beam groups whose notes span more than this (in spaces) get stems in both directions
const MIXED_BEAM_RANGE: f32 = 6.0;
/// Clefs inside the score are drawn smaller
const CHANGE_CLEF_SCALE: f32 = 0.75;
/// Room before notes preceded by a clef change or an arpeggio sign
const CLEF_CHANGE_ROOM: f32 = 2.6;
const ARPEGGIO_ROOM: f32 = 1.1;
/// Keep symbols this far inside the panel
const PANEL_MARGIN: f32 = 1.0;

/// y of a staff position (0 = bottom line, 8 = top line, odd = spaces)
pub fn pos_y(staff: usize, pos: i32) -> f32 {
    STAFF_BOTTOM[staff] - pos as f32 * 0.5
}

pub fn staff_top(staff: usize) -> f32 {
    pos_y(staff, 8)
}

/// The MIDI note a symbol stands for, to light it up while it sounds
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NoteRef {
    pub start: Duration,
    pub end: Duration,
    /// 0: right hand, 1: left hand
    pub hand: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ink {
    Plain,
    Dim,
    Note(NoteRef),
    /// Estimated fingering: in the hand's color until played
    Estimated(NoteRef),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Element {
    /// `y` is the glyph baseline, `size` 1.0 is the regular staff size
    Glyph {
        c: char,
        x: f32,
        y: f32,
        size: f32,
        ink: Ink,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        ink: Ink,
    },
    /// Slanted band; (x0, y0) - (x1, y1) is its top edge
    Beam {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        thickness: f32,
        ink: Ink,
    },
    /// Curve from x0 to x1 starting at y, bulging by `height` (negative: upwards)
    Tie {
        x0: f32,
        x1: f32,
        y: f32,
        height: f32,
        ink: Ink,
    },
    /// Dashed horizontal line
    Dashes { x0: f32, x1: f32, y: f32, ink: Ink },
    /// Vertical wavy line (rolled chord)
    Wiggle { x: f32, y0: f32, y1: f32, ink: Ink },
    /// Curve from (x0, y0) to (x1, y1) bulging by `height` (negative: upwards)
    Slur {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        height: f32,
        ink: Ink,
    },
    /// Plain text, `size` is the font height in spaces, `y` the baseline
    Text {
        text: String,
        x: f32,
        y: f32,
        size: f32,
        italic: bool,
        bold: bool,
        ink: Ink,
    },
}

/// Natural spacing of a measure
#[derive(Debug, Clone)]
pub struct MeasurePlan {
    pub natural_width: f32,
    lead: f32,
    columns: Vec<Column>,
}

#[derive(Debug, Clone)]
struct Column {
    tick: u64,
    /// Room needed left of the note heads (accidentals, displaced heads)
    left: f32,
    /// Room needed right of the note heads (dots, flags, displaced heads)
    right: f32,
    /// Duration based space to the next column
    space: f32,
}

#[derive(Debug, Clone)]
pub struct Engraved {
    pub elements: Vec<Element>,
    /// (tick, x) pairs for mapping playback position to x
    pub ticks: Vec<(u64, f32)>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Key/time signature drawn at the start of the measure
    pub signature: Signature,
    /// First measure of a half page (ties from the left get a stub, the number is shown)
    pub first_in_half: bool,
    pub last_measure: bool,
}

fn hand_index(hand: Option<Hand>, staff: usize) -> usize {
    match hand {
        Some(Hand::Right) => 0,
        Some(Hand::Left) => 1,
        None => staff,
    }
}

fn head_width(value: NoteValue, metrics: &Metrics) -> f32 {
    match value {
        NoteValue::Whole => metrics.head_whole,
        NoteValue::Half => metrics.head_half,
        _ => metrics.head_black,
    }
}

/// Staff positions of the notes (lowest first), with the 8va/8vb shift applied
fn positions(event: &Event, _staff: usize) -> Vec<i32> {
    let bottom = event.clef.bottom_line_step();
    event
        .notes
        .iter()
        .map(|n| n.step - bottom - 7 * event.ottava as i32)
        .collect()
}

fn has_second(positions: &[i32]) -> bool {
    positions.windows(2).any(|w| w[1] - w[0] == 1)
}

/// Column (0 = nearest the notes) of every accidental, stacked so they don't overlap.
/// `accidentals` are (position, glyph), in any order; returns in the same order.
fn accidental_columns(accidentals: &[(i32, char)]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..accidentals.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(accidentals[i].0));

    let mut columns: Vec<Vec<i32>> = Vec::new();
    let mut result = vec![0; accidentals.len()];
    for i in order {
        let pos = accidentals[i].0;
        let column = columns
            .iter()
            .position(|c| c.iter().all(|&p| (p - pos).abs() >= 6))
            .unwrap_or(columns.len());
        if column == columns.len() {
            columns.push(Vec::new());
        }
        columns[column].push(pos);
        result[i] = column;
    }
    result
}

fn accidental_glyph(accidental: Accidental) -> char {
    match accidental {
        Accidental::Sharp => glyphs::SHARP,
        Accidental::Flat => glyphs::FLAT,
        Accidental::Natural => glyphs::NATURAL,
        Accidental::DoubleSharp => glyphs::DOUBLE_SHARP,
        Accidental::DoubleFlat => glyphs::DOUBLE_FLAT,
    }
}

const ACCIDENTAL_COLUMN: f32 = 1.15;

fn accidentals_width(event: &Event, staff: usize) -> f32 {
    let pos = positions(event, staff);
    let list: Vec<(i32, char)> = event
        .notes
        .iter()
        .zip(pos)
        .filter_map(|(n, p)| n.accidental.map(|a| (p, accidental_glyph(a))))
        .collect();
    if list.is_empty() {
        return 0.0;
    }
    let columns = accidental_columns(&list).into_iter().max().unwrap_or(0) + 1;
    columns as f32 * ACCIDENTAL_COLUMN + 0.2
}

/// Width of a key signature (going from `previous`, whose accidentals get naturals)
pub fn key_signature_width(key: i8, previous: Option<i8>, metrics: &Metrics) -> f32 {
    let cancelled = previous.map_or(0, |p| cancelled_count(p, key));
    let n = key.unsigned_abs() as f32;
    let width = cancelled as f32 * (metrics.natural + 0.25) + n * 1.0;
    if width > 0.0 { width + 0.5 } else { 0.0 }
}

/// How many accidentals of `previous` get a natural when changing to `key`
fn cancelled_count(previous: i8, key: i8) -> u8 {
    if previous.signum() == key.signum() {
        previous.unsigned_abs().saturating_sub(key.unsigned_abs())
    } else {
        previous.unsigned_abs()
    }
}

/// How far (in positions) a key signature pattern written for the treble clef moves
/// for another clef: the bass pattern sits two positions lower, the alto one lower
fn key_shift(clef: Clef) -> i32 {
    let d = Clef::Treble.bottom_line_step() - clef.bottom_line_step();
    d - 7 * ((d as f32 / 7.0).round() as i32)
}

/// Key signature on both staves starting at x
pub fn key_signature(
    out: &mut Vec<Element>,
    x: f32,
    key: i8,
    previous: Option<i8>,
    clefs: [Clef; 2],
    metrics: &Metrics,
) -> f32 {
    // Positions on a treble staff
    const SHARPS: [i32; 7] = [8, 5, 9, 6, 3, 7, 4];
    const FLATS: [i32; 7] = [4, 7, 3, 6, 2, 5, 1];

    let mut cx = x;
    if let Some(previous) = previous {
        let cancelled = cancelled_count(previous, key) as usize;
        let pattern = if previous > 0 { &SHARPS } else { &FLATS };
        let skip = if previous.signum() == key.signum() {
            key.unsigned_abs() as usize
        } else {
            0
        };
        for &pos in pattern.iter().skip(skip).take(cancelled) {
            for staff in 0..2 {
                out.push(Element::Glyph {
                    c: glyphs::NATURAL,
                    x: cx,
                    y: pos_y(staff, pos + key_shift(clefs[staff])),
                    size: 1.0,
                    ink: Ink::Plain,
                });
            }
            cx += metrics.natural + 0.25;
        }
    }

    let (pattern, symbol) = if key >= 0 {
        (&SHARPS[..key as usize], glyphs::SHARP)
    } else {
        (&FLATS[..(-key) as usize], glyphs::FLAT)
    };
    for &pos in pattern {
        for staff in 0..2 {
            out.push(Element::Glyph {
                c: symbol,
                x: cx,
                y: pos_y(staff, pos + key_shift(clefs[staff])),
                size: 1.0,
                ink: Ink::Plain,
            });
        }
        cx += 1.0;
    }
    key_signature_width(key, previous, metrics)
}

pub fn time_signature_width(time: (u8, u8), metrics: &Metrics) -> f32 {
    let digits = time.0.max(time.1).to_string().len() as f32;
    digits * metrics.time_digit + 0.8
}

/// Time signature on both staves starting at x
pub fn time_signature(out: &mut Vec<Element>, x: f32, time: (u8, u8), metrics: &Metrics) -> f32 {
    let width = time_signature_width(time, metrics);
    let widest = time.0.max(time.1).to_string().len() as f32;
    for staff in 0..2 {
        for (number, pos) in [(time.0, 6), (time.1, 2)] {
            let text = number.to_string();
            let offset = (widest - text.len() as f32) * metrics.time_digit / 2.0;
            for (i, d) in text.chars().filter_map(|c| c.to_digit(10)).enumerate() {
                out.push(Element::Glyph {
                    c: glyphs::time_sig_digit(d),
                    x: x + offset + i as f32 * metrics.time_digit,
                    y: pos_y(staff, pos),
                    size: 1.0,
                    ink: Ink::Plain,
                });
            }
        }
    }
    width
}

/// Key (new key, key it replaces) and time signature shown at the start of a measure
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Signature {
    pub key: Option<(i8, Option<i8>)>,
    pub time: Option<(u8, u8)>,
    /// Clefs to show at the start (when they differ from what the reader last saw)
    pub clefs: [Option<Clef>; 2],
}

impl Signature {
    /// The changes that happen at this measure
    pub fn changes(measure: &Measure) -> Self {
        Self {
            key: measure.previous_key.map(|p| (measure.key, Some(p))),
            time: measure
                .time_signature_changed
                .then_some(measure.time_signature),
            clefs: [None, None],
        }
    }

    fn width(&self, metrics: &Metrics) -> f32 {
        let key = self.key.map_or(0.0, |(key, previous)| {
            key_signature_width(key, previous, metrics)
        });
        let time = self
            .time
            .map_or(0.0, |time| time_signature_width(time, metrics));
        let clef = if self.clefs.iter().any(Option::is_some) {
            metrics.clef * CHANGE_CLEF_SCALE + 0.6
        } else {
            0.0
        };
        key + time + clef
    }
}

/// Space for a duration: grows with the logarithm of the length
fn duration_space(ticks: u64, ppq: u16) -> f32 {
    let quarters = ticks as f32 / ppq as f32;
    (3.6 * quarters.powf(0.55)).max(1.9)
}

fn events_at(measure: &Measure) -> impl Iterator<Item = (usize, usize, &Event, &Voice)> {
    measure.staves.iter().enumerate().flat_map(|(staff, s)| {
        s.voices.iter().flat_map(move |voice| {
            voice
                .events
                .iter()
                .filter(|e| !e.whole_measure_rest)
                .map(move |e| (staff, s.voices.len(), e, voice))
        })
    })
}

fn is_beamed(voice: &Voice, event: &Event) -> bool {
    let index = voice.events.iter().position(|e| std::ptr::eq(e, event));
    index.is_some_and(|i| voice.beams.iter().any(|b| b.first <= i && i <= b.last))
}

pub fn plan(score: &Score, index: usize, metrics: &Metrics, signature: Signature) -> MeasurePlan {
    let measure = &score.measures[index];
    let lead = 1.0 + signature.width(metrics) + if measure.repeat_start { 1.6 } else { 0.0 };

    let mut ticks: Vec<u64> = events_at(measure).map(|(_, _, e, _)| e.tick).collect();
    ticks.sort_unstable();
    ticks.dedup();

    let mut columns: Vec<Column> = ticks
        .iter()
        .map(|&tick| Column {
            tick,
            left: 0.0,
            right: 0.0,
            space: 0.0,
        })
        .collect();

    for (staff, _, event, voice) in events_at(measure) {
        let Some(column) = columns.iter_mut().find(|c| c.tick == event.tick) else {
            continue;
        };
        let head = head_width(event.value, metrics);
        let mut left = 0.0;
        let mut right = 0.0;
        if !event.is_rest() {
            left += accidentals_width(event, staff);
            left += event.grace.len() as f32 * GRACE_SPACING;
            if has_second(&positions(event, staff)) {
                left += head;
                right += head;
            }
            if event.value.flags() > 0 && !is_beamed(voice, event) {
                right += metrics.flag * 0.8;
            }
            if event.arpeggio {
                left += ARPEGGIO_ROOM;
            }
        }
        if event.dots > 0 {
            right += 0.35 + event.dots as f32 * 0.5;
        }
        column.left = column.left.max(left);
        column.right = column.right.max(right);
    }

    // Clef changes need room before the first column at or after them
    for staff in &measure.staves {
        for &(tick, _) in &staff.clef_changes {
            if let Some(c) = columns.iter_mut().find(|c| c.tick >= tick) {
                c.left += CLEF_CHANGE_ROOM;
            }
        }
    }

    for i in 0..columns.len() {
        let next = columns.get(i + 1).map_or(measure.end_tick, |c| c.tick);
        columns[i].space = duration_space(next - columns[i].tick, score.ppq);
    }

    let natural_width = if columns.is_empty() {
        // Only whole measure rests
        lead + 7.0
    } else {
        let mut x = lead + columns[0].left;
        for i in 0..columns.len() {
            x += advance(&columns, i, metrics, 0.0);
        }
        x + 0.5 + if measure.repeat_end { 1.2 } else { 0.0 }
    };

    MeasurePlan {
        natural_width,
        lead,
        columns,
    }
}

/// Distance from column i to the next one (or to the end of the measure)
fn advance(columns: &[Column], i: usize, metrics: &Metrics, stretch: f32) -> f32 {
    let c = &columns[i];
    let needed = match columns.get(i + 1) {
        Some(next) => metrics.head_black + c.right + 0.4 + next.left,
        None => metrics.head_black + c.right + 0.8,
    };
    c.space.max(needed) + stretch * c.space
}

#[derive(Clone)]
struct Chord {
    /// x of the note heads (undisplaced)
    x: f32,
    up: bool,
    stem_x: f32,
    /// Where the stem touches the chord
    base: f32,
    /// y of the note nearest to the stem tip
    outer: f32,
    tip: f32,
    flags: u8,
    head: f32,
    ink: Ink,
}

/// Direction by the note farthest from the middle line (ties go down)
fn stem_up_for(positions: &[i32]) -> bool {
    let above = positions.iter().max().copied().unwrap_or(4) - 4;
    let below = 4 - positions.iter().min().copied().unwrap_or(4);
    below > above
}

pub fn engrave(
    score: &Score,
    index: usize,
    plan: &MeasurePlan,
    width: f32,
    metrics: &Metrics,
    options: Options,
) -> Engraved {
    let measure = &score.measures[index];
    let mut out = Vec::new();

    // Column positions, stretched to the requested width
    let total_space: f32 = plan.columns.iter().map(|c| c.space).sum();
    let stretch = if total_space > 0.0 {
        (width - plan.natural_width) / total_space
    } else {
        0.0
    };
    let squeeze = if width < plan.natural_width {
        width / plan.natural_width
    } else {
        1.0
    };
    let stretch = stretch.max(0.0);

    let mut column_x = Vec::with_capacity(plan.columns.len());
    if let Some(first) = plan.columns.first() {
        let mut x = plan.lead + first.left;
        for i in 0..plan.columns.len() {
            column_x.push(x * squeeze);
            x += advance(&plan.columns, i, metrics, stretch);
        }
    }
    let x_of = |tick: u64| -> f32 {
        plan.columns
            .iter()
            .position(|c| c.tick == tick)
            .map_or(width / 2.0, |i| column_x[i])
    };

    let mut ticks = vec![(measure.start_tick, 0.0)];
    for (c, &x) in plan.columns.iter().zip(&column_x) {
        ticks.push((c.tick, x + metrics.head_black / 2.0));
    }
    ticks.push((measure.end_tick, width));

    let start_clefs = [measure.staves[0].clef, measure.staves[1].clef];

    // Repeat start: thick and thin line with dots
    let mut x = 0.6;
    if measure.repeat_start {
        repeat_sign(&mut out, 0.0, true);
        x += 1.6;
    }

    // Clef / key / time changes at the start
    {
        if options.signature.clefs.iter().any(Option::is_some) {
            for (staff, clef) in options.signature.clefs.iter().enumerate() {
                if let Some(clef) = clef {
                    let (c, line) = glyphs::clef(*clef);
                    out.push(Element::Glyph {
                        c,
                        x,
                        y: pos_y(staff, line),
                        size: CHANGE_CLEF_SCALE,
                        ink: Ink::Plain,
                    });
                }
            }
            x += metrics.clef * CHANGE_CLEF_SCALE + 0.6;
        }
        if let Some((key, previous)) = options.signature.key {
            x += key_signature(&mut out, x, key, previous, start_clefs, metrics);
        }
        if let Some(time) = options.signature.time {
            time_signature(&mut out, x, time, metrics);
        }
    }

    // Clef changes inside the measure, just before the notes they apply to
    for (staff, staff_measure) in measure.staves.iter().enumerate() {
        for &(tick, clef) in &staff_measure.clef_changes {
            let Some(i) = plan.columns.iter().position(|c| c.tick >= tick) else {
                continue;
            };
            let (c, line) = glyphs::clef(clef);
            out.push(Element::Glyph {
                c,
                x: column_x[i] - plan.columns[i].left + 0.2,
                y: pos_y(staff, line),
                size: CHANGE_CLEF_SCALE,
                ink: Ink::Plain,
            });
        }
    }

    // Volta bracket
    if let Some(label) = &measure.ending {
        let y = staff_top(0) - 3.0;
        out.push(Element::Rect {
            x: 0.3,
            y,
            w: width - 0.8,
            h: 0.12,
            ink: Ink::Plain,
        });
        out.push(Element::Rect {
            x: 0.3,
            y,
            w: 0.12,
            h: 1.4,
            ink: Ink::Plain,
        });
        out.push(Element::Text {
            text: label.clone(),
            x: 0.7,
            y: y + 1.3,
            size: 1.3,
            italic: false,
            bold: false,
            ink: Ink::Plain,
        });
    }

    // Measure number
    if options.first_in_half {
        let number = measure.number.clone();
        for (i, d) in number.chars().filter_map(|c| c.to_digit(10)).enumerate() {
            out.push(Element::Glyph {
                c: glyphs::time_sig_digit(d),
                x: 0.2 + i as f32 * metrics.time_digit * 0.4,
                y: staff_top(0) - 0.7,
                size: 0.4,
                ink: Ink::Dim,
            });
        }
    }

    // Chord geometry of every voice, for slurs
    let mut geometry: [Vec<Vec<Option<Chord>>>; 2] = [Vec::new(), Vec::new()];
    for staff in 0..2 {
        let voices = &measure.staves[staff].voices;
        for (v, voice) in voices.iter().enumerate() {
            // With several voices, even ones point up, odd ones down
            let forced = (voices.len() >= 2).then_some(v % 2 == 0);
            let others: Vec<&Voice> = voices
                .iter()
                .enumerate()
                .filter(|(o, _)| *o != v)
                .map(|(_, other)| other)
                .collect();
            let chords = engrave_voice(
                &mut out, voice, &others, staff, v, forced, &x_of, width, metrics, options,
            );
            geometry[staff].push(chords);
        }
    }

    slurs(&mut out, score, index, &geometry, width);
    wedges(&mut out, score, index, &ticks, width);
    directions(&mut out, measure, &ticks);

    let lowest = out
        .iter()
        .map(|e| match *e {
            Element::Glyph { y, .. } => y + 0.6,
            Element::Rect { y, h, .. } => y + h,
            Element::Beam {
                y0, y1, thickness, ..
            } => y0.max(y1) + thickness,
            Element::Tie { y, height, .. } => y + height.max(0.0),
            Element::Dashes { y, .. } => y + 0.3,
            Element::Wiggle { y1, .. } => y1,
            Element::Slur { y0, y1, height, .. } => y0.max(y1) + height.max(0.0),
            Element::Text { y, .. } => y + 0.4,
        })
        .fold(STAFF_BOTTOM[1], f32::max);
    let pedal_y = (lowest + 1.3)
        .max(STAFF_BOTTOM[1] + 2.8)
        .min(PANEL_HEIGHT - 0.4);
    pedal(&mut out, measure, &ticks, width, pedal_y);

    // Barline
    let top = staff_top(0);
    let bottom = STAFF_BOTTOM[1];
    if measure.repeat_end {
        repeat_sign(&mut out, width, false);
    } else if options.last_measure {
        out.push(Element::Rect {
            x: width - 1.0,
            y: top,
            w: BARLINE_WIDTH,
            h: bottom - top,
            ink: Ink::Plain,
        });
        out.push(Element::Rect {
            x: width - 0.5,
            y: top,
            w: 0.5,
            h: bottom - top,
            ink: Ink::Plain,
        });
    } else {
        out.push(Element::Rect {
            x: width - BARLINE_WIDTH,
            y: top,
            w: BARLINE_WIDTH,
            h: bottom - top,
            ink: Ink::Plain,
        });
    }

    Engraved {
        elements: out,
        ticks,
    }
}

#[allow(clippy::too_many_arguments)]
fn engrave_voice(
    out: &mut Vec<Element>,
    voice: &Voice,
    others: &[&Voice],
    staff: usize,
    v: usize,
    forced_up: Option<bool>,
    x_of: &dyn Fn(u64) -> f32,
    width: f32,
    metrics: &Metrics,
    options: Options,
) -> Vec<Option<Chord>> {
    // Stem direction: as written in the score, fixed with several voices, per beam
    // group or per chord otherwise
    let mut up: Vec<bool> = voice
        .events
        .iter()
        .map(|e| {
            e.stem
                .or(forced_up)
                .unwrap_or_else(|| stem_up_for(&positions(e, staff)))
        })
        .collect();
    // Beams spanning a very wide range sit between the notes, stems pointing at them
    let mut mixed: Vec<Option<f32>> = vec![None; voice.beams.len()];
    for (b, beam) in voice.beams.iter().enumerate() {
        let all: Vec<i32> = voice.events[beam.first..=beam.last]
            .iter()
            .flat_map(|e| positions(e, staff))
            .collect();
        let (Some(&low), Some(&high)) = (all.iter().min(), all.iter().max()) else {
            continue;
        };
        let middle = (pos_y(staff, low) + pos_y(staff, high)) / 2.0;
        let given: Vec<bool> = voice.events[beam.first..=beam.last]
            .iter()
            .filter(|e| !e.is_rest())
            .filter_map(|e| e.stem)
            .collect();
        if let Some(&first) = given.first() {
            if given.iter().all(|&s| s == first) {
                up[beam.first..=beam.last]
                    .iter_mut()
                    .for_each(|u| *u = first);
            } else {
                // The score points the stems both ways: a beam between the notes
                mixed[b] = Some(middle);
            }
            continue;
        }
        if forced_up.is_none() {
            if pos_y(staff, low) - pos_y(staff, high) > MIXED_BEAM_RANGE {
                mixed[b] = Some(middle);
                for i in beam.first..=beam.last {
                    let chord_low = positions(&voice.events[i], staff).first().copied();
                    // Chords below the beam point up to it
                    up[i] = chord_low.is_some_and(|p| pos_y(staff, p) > middle);
                }
            } else {
                let dir = stem_up_for(&all);
                up[beam.first..=beam.last].iter_mut().for_each(|u| *u = dir);
            }
        }
    }

    let mut chords: Vec<Option<Chord>> = Vec::with_capacity(voice.events.len());
    for (i, event) in voice.events.iter().enumerate() {
        if event.is_rest() {
            rest(out, event, others, staff, forced_up, x_of, width);
            chords.push(None);
        } else {
            let x = x_of(event.tick);
            chords.push(Some(chord(out, event, staff, v, up[i], x, metrics)));
        }
    }

    // Beams set the stem tips of their chords
    for (beam, mixed) in voice.beams.iter().zip(&mixed) {
        match mixed {
            Some(middle) => mixed_beam_group(out, beam.first, beam.last, &mut chords, *middle),
            None => beam_group(out, voice, beam.first, beam.last, &mut chords, staff),
        }
    }

    // Stems and flags
    let beamed = |i: usize| voice.beams.iter().any(|b| b.first <= i && i <= b.last);
    for (i, chord) in chords.iter().enumerate() {
        let Some(chord) = chord else { continue };
        let event = &voice.events[i];
        if !event.value.has_stem() {
            continue;
        }
        let (top, bottom) = if chord.base < chord.tip {
            (chord.base, chord.tip)
        } else {
            (chord.tip, chord.base)
        };
        out.push(Element::Rect {
            x: chord.stem_x,
            y: top,
            w: STEM_WIDTH,
            h: bottom - top,
            ink: chord.ink,
        });
        if chord.flags > 0 && !beamed(i) {
            let flag = if chord.up {
                glyphs::FLAG_UP[chord.flags as usize - 1]
            } else {
                glyphs::FLAG_DOWN[chord.flags as usize - 1]
            };
            out.push(Element::Glyph {
                c: flag,
                x: chord.stem_x,
                y: chord.tip,
                size: 1.0,
                ink: chord.ink,
            });
        }
    }

    ties(out, voice, staff, &chords, width, metrics, options);
    // One 8va line per staff: skip the lower of two voices
    if forced_up != Some(false) {
        ottava_lines(out, voice, staff, &chords, x_of, metrics);
    }
    tuplets(out, voice, staff, &chords, metrics);
    chords
}

fn chord(
    out: &mut Vec<Element>,
    event: &Event,
    staff: usize,
    voice: usize,
    up: bool,
    x: f32,
    metrics: &Metrics,
) -> Chord {
    let pos = positions(event, staff);
    let head = head_width(event.value, metrics);
    let glyph = match event.value {
        NoteValue::Whole => glyphs::NOTEHEAD_WHOLE,
        NoteValue::Half => glyphs::NOTEHEAD_HALF,
        _ => glyphs::NOTEHEAD_BLACK,
    };

    // Heads a second apart go on opposite sides of the stem
    let mut offsets = vec![0.0f32; pos.len()];
    if up {
        for i in 1..pos.len() {
            if pos[i] - pos[i - 1] == 1 && offsets[i - 1] == 0.0 {
                offsets[i] = head - STEM_WIDTH;
            }
        }
    } else {
        for i in (0..pos.len().saturating_sub(1)).rev() {
            if pos[i + 1] - pos[i] == 1 && offsets[i + 1] == 0.0 {
                offsets[i] = -(head - STEM_WIDTH);
            }
        }
    }
    let left = offsets.iter().copied().fold(0.0, f32::min);
    let right = offsets.iter().copied().fold(0.0, f32::max);

    let note_ink = |i: usize| {
        let n = &event.notes[i];
        Ink::Note(NoteRef {
            start: n.start,
            end: n.end,
            hand: hand_index(n.hand, staff),
        })
    };
    let chord_ink = note_ink(pos.len() - 1);

    // Ledger lines
    let lowest = pos[0];
    let highest = *pos.last().unwrap();
    let ledger_x = x + left - LEDGER_EXTENSION;
    let ledger_w = right - left + head + 2.0 * LEDGER_EXTENSION;
    for p in (10..=highest).step_by(2) {
        out.push(Element::Rect {
            x: ledger_x,
            y: pos_y(staff, p) - LEDGER_WIDTH / 2.0,
            w: ledger_w,
            h: LEDGER_WIDTH,
            ink: Ink::Plain,
        });
    }
    let mut p = -2;
    while p >= lowest {
        out.push(Element::Rect {
            x: ledger_x,
            y: pos_y(staff, p) - LEDGER_WIDTH / 2.0,
            w: ledger_w,
            h: LEDGER_WIDTH,
            ink: Ink::Plain,
        });
        p -= 2;
    }

    // Grace notes: small slashed notes before the chord (and its accidentals)
    let accidental_room = accidentals_width(event, staff);
    for (g, grace) in event.grace.iter().rev().enumerate() {
        let bottom = event.clef.bottom_line_step();
        let p = grace.step - bottom - 7 * event.ottava as i32;
        let gx = x + left - accidental_room - (g + 1) as f32 * GRACE_SPACING;
        let ink = Ink::Note(NoteRef {
            start: grace.start,
            end: grace.end,
            hand: hand_index(grace.hand, staff),
        });
        let mut ledger = if p >= 10 { 10 } else { -2 };
        while (p >= 10 && ledger <= p) || (p <= -2 && ledger >= p) {
            out.push(Element::Rect {
                x: gx - 0.25,
                y: pos_y(staff, ledger) - LEDGER_WIDTH / 2.0,
                w: GRACE_SCALE * head + 0.5,
                h: LEDGER_WIDTH,
                ink: Ink::Plain,
            });
            ledger += if p >= 10 { 2 } else { -2 };
        }
        out.push(Element::Glyph {
            c: glyphs::GRACE_ACCIACCATURA,
            x: gx,
            y: pos_y(staff, p),
            size: GRACE_SCALE,
            ink,
        });
        if let Some(accidental) = grace.accidental {
            let c = accidental_glyph(accidental);
            out.push(Element::Glyph {
                c,
                x: gx - metrics.accidental(c) * GRACE_SCALE - 0.15,
                y: pos_y(staff, p),
                size: GRACE_SCALE,
                ink,
            });
        }
    }

    // Heads
    for (i, &p) in pos.iter().enumerate() {
        out.push(Element::Glyph {
            c: glyph,
            x: x + offsets[i],
            y: pos_y(staff, p),
            size: 1.0,
            ink: note_ink(i),
        });
    }

    // Accidentals, stacked leftwards
    let list: Vec<(usize, i32, char)> = event
        .notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.accidental.map(|a| (i, pos[i], accidental_glyph(a))))
        .collect();
    let columns = accidental_columns(&list.iter().map(|&(_, p, c)| (p, c)).collect::<Vec<_>>());
    for (&(i, p, c), column) in list.iter().zip(columns) {
        let w = metrics.accidental(c);
        out.push(Element::Glyph {
            c,
            x: x + left - 0.2 - column as f32 * ACCIDENTAL_COLUMN - w,
            y: pos_y(staff, p),
            size: 1.0,
            ink: note_ink(i),
        });
    }

    // Dots sit in spaces; in a lower voice, dots of notes on lines go below
    if event.dots > 0 {
        let lower_voice = voice == 1;
        for (i, &p) in pos.iter().enumerate() {
            let dot_pos = if p % 2 == 0 {
                if lower_voice { p - 1 } else { p + 1 }
            } else {
                p
            };
            for d in 0..event.dots {
                out.push(Element::Glyph {
                    c: glyphs::DOT,
                    x: x + right + head + 0.3 + d as f32 * 0.5,
                    y: pos_y(staff, dot_pos),
                    size: 1.0,
                    ink: note_ink(i),
                });
            }
        }
    }

    // Staccato on the head side
    if event.staccato {
        let (glyph, p) = if up {
            let mut p = lowest - 2;
            if (0..=8).contains(&p) && p % 2 == 0 {
                p -= 1;
            }
            (glyphs::STACCATO_BELOW, p)
        } else {
            let mut p = highest + 2;
            if (0..=8).contains(&p) && p % 2 == 0 {
                p += 1;
            }
            (glyphs::STACCATO_ABOVE, p)
        };
        out.push(Element::Glyph {
            c: glyph,
            x: x + head / 2.0 - 0.2,
            y: pos_y(staff, p),
            size: 1.0,
            ink: chord_ink,
        });
    }

    // Other articulations stack outwards on the head side; fermatas go above
    let mut stacked = if event.staccato { 2 } else { 0 };
    for articulation in &event.articulations {
        let (pair, always_above) = match articulation {
            Articulation::Accent => (glyphs::ACCENT, false),
            Articulation::Tenuto => (glyphs::TENUTO, false),
            Articulation::Marcato => (glyphs::MARCATO, true),
            Articulation::Staccatissimo => (glyphs::STACCATISSIMO, false),
            Articulation::Fermata => (glyphs::FERMATA, true),
        };
        let (c, p) = if always_above {
            (pair.0, highest.max(8) + 3 + stacked)
        } else if up {
            let mut p = lowest - 2 - stacked;
            if (0..=8).contains(&p) && p % 2 == 0 {
                p -= 1;
            }
            (pair.1, p)
        } else {
            let mut p = highest + 2 + stacked;
            if (0..=8).contains(&p) && p % 2 == 0 {
                p += 1;
            }
            (pair.0, p)
        };
        out.push(Element::Glyph {
            c,
            x: x + head / 2.0 - 0.55,
            y: pos_y(staff, p),
            size: 1.0,
            ink: chord_ink,
        });
        stacked += 2;
    }

    // Ornaments above the staff
    if let Some(ornament) = event.ornament {
        let c = match ornament {
            Ornament::Trill => glyphs::TRILL,
            Ornament::Turn => glyphs::TURN,
            Ornament::InvertedTurn => glyphs::INVERTED_TURN,
            Ornament::Mordent => glyphs::MORDENT,
            Ornament::InvertedMordent => glyphs::SHORT_TRILL,
        };
        out.push(Element::Glyph {
            c,
            x: x + head / 2.0 - 0.7,
            y: pos_y(staff, highest.max(8) + 4),
            size: 1.0,
            ink: chord_ink,
        });
    }

    // Fingering: above the chord on the upper staff, below it on the lower one
    let fingerings: Vec<(usize, &str)> = event
        .notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.fingering.as_deref().map(|f| (i, f)))
        .collect();
    for (k, (i, text)) in fingerings.iter().rev().enumerate() {
        let ink = match note_ink(*i) {
            Ink::Note(note) if event.notes[*i].fingering_auto => Ink::Estimated(note),
            ink => ink,
        };
        let y = if staff == 0 {
            pos_y(staff, highest.max(8) + 3) - k as f32 * 1.1
        } else {
            pos_y(staff, lowest.min(0) - 4) + (fingerings.len() - 1 - k) as f32 * 1.1
        };
        for (j, d) in text.chars().filter_map(|c| c.to_digit(10)).enumerate() {
            out.push(Element::Glyph {
                c: glyphs::fingering_digit(d),
                x: x + head / 2.0 - 0.35 + j as f32 * 0.7,
                y,
                size: 0.8,
                ink,
            });
        }
    }

    // Rolled chord: wavy line left of everything else
    if event.arpeggio {
        let graces = event.grace.len() as f32 * GRACE_SPACING;
        out.push(Element::Wiggle {
            x: x + left - accidental_room - graces - ARPEGGIO_ROOM + 0.2,
            y0: pos_y(staff, highest) - 0.6,
            y1: pos_y(staff, lowest) + 0.6,
            ink: chord_ink,
        });
    }

    // Trill sign above the staff
    if event.notes.iter().any(|n| n.trill) {
        out.push(Element::Glyph {
            c: glyphs::TRILL,
            x: x + head / 2.0 - 0.7,
            y: pos_y(staff, highest.max(8) + 4),
            size: 1.0,
            ink: chord_ink,
        });
    }

    // Stem geometry (drawn later, beams may change the tip)
    let flags = event.value.flags();
    let extra = match flags {
        2 => 0.25,
        3 => 0.75,
        4 => 1.25,
        _ => 0.0,
    };
    let middle = pos_y(staff, 4);
    let (stem_x, base, outer, tip) = if up {
        let outer = pos_y(staff, highest);
        let tip = (outer - STEM_LENGTH - extra).min(middle).max(PANEL_MARGIN);
        (
            x + head - STEM_WIDTH,
            pos_y(staff, lowest) - 0.17,
            outer,
            tip,
        )
    } else {
        let outer = pos_y(staff, lowest);
        let tip = (outer + STEM_LENGTH + extra)
            .max(middle)
            .min(PANEL_HEIGHT - PANEL_MARGIN);
        (x, pos_y(staff, highest) + 0.17, outer, tip)
    };

    Chord {
        x,
        up,
        stem_x,
        base,
        outer,
        tip,
        flags,
        head,
        ink: chord_ink,
    }
}

fn rest(
    out: &mut Vec<Element>,
    event: &Event,
    others: &[&Voice],
    staff: usize,
    forced_up: Option<bool>,
    x_of: &dyn Fn(u64) -> f32,
    width: f32,
) {
    let (c, mut pos) = match event.value {
        // The whole rest hangs from the 4th line, the half rest sits on the middle one
        NoteValue::Whole => (glyphs::REST_WHOLE, 6),
        NoteValue::Half => (glyphs::REST_HALF, 4),
        NoteValue::Quarter => (glyphs::REST_QUARTER, 4),
        NoteValue::Eighth => (glyphs::REST_8TH, 4),
        NoteValue::Sixteenth => (glyphs::REST_16TH, 4),
        NoteValue::ThirtySecond => (glyphs::REST_32ND, 4),
        NoteValue::SixtyFourth => (glyphs::REST_64TH, 4),
    };
    // With two voices, rests move out of the way of the other voice's notes
    // sounding at the same time
    let (start, end) = (event.tick, event.tick + event.ticks);
    let overlapping: Vec<i32> = others
        .iter()
        .flat_map(|v| v.events.iter())
        .filter(|e| e.tick < end && e.tick + e.ticks > start)
        .flat_map(|e| positions(e, staff))
        .collect();
    // Longer rests are drawn on lines
    let even = |p: i32| if pos % 2 == 0 { p + p.rem_euclid(2) } else { p };
    match forced_up {
        Some(true) => {
            let above = overlapping.iter().max().map_or(8, |&p| p + 4);
            pos = even((pos + 4).max(above));
        }
        Some(false) => {
            let below = overlapping.iter().min().map_or(0, |&p| p - 4);
            pos = -even(-(pos - 4).min(below));
        }
        None => {}
    }
    let x = if event.whole_measure_rest {
        width / 2.0 - 0.7
    } else {
        x_of(event.tick)
    };
    out.push(Element::Glyph {
        c,
        x,
        y: pos_y(staff, pos),
        size: 1.0,
        ink: Ink::Dim,
    });
    for d in 0..event.dots {
        out.push(Element::Glyph {
            c: glyphs::DOT,
            x: x + 1.5 + d as f32 * 0.5,
            y: pos_y(staff, pos + 1),
            size: 1.0,
            ink: Ink::Dim,
        });
    }
}

/// A beam is lit from the start of its first chord to the end of its last one
fn group_ink(first: Ink, last: Ink) -> Ink {
    match (first, last) {
        (Ink::Note(a), Ink::Note(b)) => Ink::Note(NoteRef {
            start: a.start,
            end: b.end.max(a.end),
            hand: a.hand,
        }),
        (ink, _) => ink,
    }
}

/// Horizontal beam between high and low notes; stems point at it from both sides
fn mixed_beam_group(
    out: &mut Vec<Element>,
    first: usize,
    last: usize,
    chords: &mut [Option<Chord>],
    middle: f32,
) {
    let members: Vec<usize> = (first..=last).filter(|&i| chords[i].is_some()).collect();
    if members.len() < 2 {
        return;
    }
    let levels = members
        .iter()
        .map(|&i| chords[i].as_ref().unwrap().flags)
        .max()
        .unwrap_or(1)
        .max(1);
    // Primary beam on top, the others stacked below it
    let top = middle - BEAM_THICKNESS / 2.0;
    let bottom = top + BEAM_DISTANCE * (levels - 1) as f32 + BEAM_THICKNESS;

    for &i in &members {
        let chord = chords[i].as_mut().unwrap();
        chord.tip = if chord.up { top } else { bottom };
    }

    let ink = group_ink(
        chords[members[0]].as_ref().unwrap().ink,
        chords[*members.last().unwrap()].as_ref().unwrap().ink,
    );
    let x_of = |i: usize| chords[i].as_ref().unwrap().stem_x;
    let flags_of = |i: usize| chords[i].as_ref().unwrap().flags;
    let x0 = x_of(members[0]);
    let x1 = x_of(*members.last().unwrap()) + STEM_WIDTH;
    out.push(Element::Beam {
        x0,
        y0: top,
        x1,
        y1: top,
        thickness: BEAM_THICKNESS,
        ink,
    });
    for level in 2..=levels {
        let y = top + BEAM_DISTANCE * (level - 1) as f32;
        let mut k = 0;
        while k < members.len() {
            if flags_of(members[k]) < level {
                k += 1;
                continue;
            }
            let mut j = k;
            while j + 1 < members.len() && flags_of(members[j + 1]) >= level {
                j += 1;
            }
            let xa = x_of(members[k]);
            let (a, b) = if j > k {
                (xa, x_of(members[j]) + STEM_WIDTH)
            } else if k + 1 < members.len() {
                (xa, xa + 1.1)
            } else {
                (xa - 1.1 + STEM_WIDTH, xa + STEM_WIDTH)
            };
            out.push(Element::Beam {
                x0: a,
                y0: y,
                x1: b,
                y1: y,
                thickness: BEAM_THICKNESS,
                ink,
            });
            k = j + 1;
        }
    }
}

fn beam_group(
    out: &mut Vec<Element>,
    voice: &Voice,
    first: usize,
    last: usize,
    chords: &mut [Option<Chord>],
    staff: usize,
) {
    let members: Vec<usize> = (first..=last).filter(|&i| chords[i].is_some()).collect();
    if members.len() < 2 {
        return;
    }
    struct Member {
        up: bool,
        stem_x: f32,
        outer: f32,
        flags: u8,
        ink: Ink,
    }
    let data: HashMap<usize, Member> = members
        .iter()
        .map(|&i| {
            let c = chords[i].as_ref().unwrap();
            (
                i,
                Member {
                    up: c.up,
                    stem_x: c.stem_x,
                    outer: c.outer,
                    flags: c.flags,
                    ink: c.ink,
                },
            )
        })
        .collect();
    let get = |i: usize| &data[&i];
    let up = get(members[0]).up;
    let x0 = get(members[0]).stem_x;
    let x1 = get(*members.last().unwrap()).stem_x;
    let dir = if up { -1.0 } else { 1.0 };

    let max_flags = members.iter().map(|&i| get(i).flags).max().unwrap_or(1);
    let min_len = STEM_LENGTH - 0.25 + (max_flags.saturating_sub(2)) as f32 * BEAM_DISTANCE;

    // Slope from the outer notes at the ends, at most one space overall; flat when
    // an inner note sticks out further than both ends
    let first_outer = get(members[0]).outer;
    let last_outer = get(*members.last().unwrap()).outer;
    let inner_extreme = members[1..members.len() - 1].iter().any(|&i| {
        let o = get(i).outer;
        if up {
            o < first_outer.min(last_outer)
        } else {
            o > first_outer.max(last_outer)
        }
    });
    let span = (x1 - x0).max(0.01);
    let slope = if inner_extreme {
        0.0
    } else {
        ((last_outer - first_outer).clamp(-1.0, 1.0)) / span
    };

    // Offset so that every stem is long enough
    let mut c = if up { f32::MAX } else { f32::MIN };
    for &i in &members {
        let chord = get(i);
        let needed = chord.outer + dir * min_len - slope * (chord.stem_x - x0);
        c = if up { c.min(needed) } else { c.max(needed) };
    }
    // Beams reach at least the middle line
    let middle = pos_y(staff, 4);
    let tips: Vec<f32> = members
        .iter()
        .map(|&i| c + slope * (get(i).stem_x - x0))
        .collect();
    if up {
        let highest_tip = tips.iter().copied().fold(f32::MAX, f32::min);
        if highest_tip > middle {
            c -= highest_tip - middle;
        }
    } else {
        let lowest_tip = tips.iter().copied().fold(f32::MIN, f32::max);
        if lowest_tip < middle {
            c += middle - lowest_tip;
        }
    }

    // Stay inside the panel, shortening the stems if needed
    let tips: Vec<f32> = members
        .iter()
        .map(|&i| c + slope * (get(i).stem_x - x0))
        .collect();
    let (min_tip, max_tip) = tips
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &t| (a.min(t), b.max(t)));
    if up && min_tip < PANEL_MARGIN {
        c += PANEL_MARGIN - min_tip;
    } else if !up && max_tip > PANEL_HEIGHT - PANEL_MARGIN {
        c -= max_tip - (PANEL_HEIGHT - PANEL_MARGIN);
    }

    let line = |x: f32| c + slope * (x - x0);
    for &i in &members {
        let chord = chords[i].as_mut().unwrap();
        chord.tip = line(chord.stem_x);
    }

    let ink = group_ink(get(members[0]).ink, get(*members.last().unwrap()).ink);
    let band = |out: &mut Vec<Element>, xa: f32, xb: f32, level: u8| {
        let offset = -dir * BEAM_DISTANCE * (level - 1) as f32;
        // Top edge: the beam grows from the tip towards the notes
        let top = |x: f32| line(x) + offset - if up { 0.0 } else { BEAM_THICKNESS };
        out.push(Element::Beam {
            x0: xa,
            y0: top(xa),
            x1: xb,
            y1: top(xb),
            thickness: BEAM_THICKNESS,
            ink,
        });
    };

    band(out, x0, x1 + STEM_WIDTH, 1);

    // Secondary beams: runs of notes with that many flags, partial beams for loners
    for level in 2..=3u8 {
        let mut k = 0;
        while k < members.len() {
            if get(members[k]).flags < level {
                k += 1;
                continue;
            }
            let mut j = k;
            while j + 1 < members.len() && get(members[j + 1]).flags >= level {
                j += 1;
            }
            let xa = get(members[k]).stem_x;
            if j > k {
                band(out, xa, get(members[j]).stem_x + STEM_WIDTH, level);
            } else {
                let hook = 1.1;
                // Point towards the neighbour inside the group (backwards at the end)
                let (a, b) = if k + 1 < members.len() {
                    (xa, xa + hook)
                } else {
                    (xa - hook + STEM_WIDTH, xa + STEM_WIDTH)
                };
                band(out, a, b, level);
            }
            k = j + 1;
        }
    }
    let _ = voice;
}

fn ties(
    out: &mut Vec<Element>,
    voice: &Voice,
    staff: usize,
    chords: &[Option<Chord>],
    width: f32,
    metrics: &Metrics,
    options: Options,
) {
    for (i, event) in voice.events.iter().enumerate() {
        let Some(chord) = &chords[i] else { continue };
        let pos = positions(event, staff);
        let n = pos.len();
        for (k, note) in event.notes.iter().enumerate() {
            // Outer half of a chord curves away from the middle, single notes away from the stem
            let down = if n > 1 { k < n / 2 } else { chord.up };
            let y = pos_y(staff, pos[k]) + if down { 0.6 } else { -0.6 };
            let ink = Ink::Note(NoteRef {
                start: note.start,
                end: note.end,
                hand: hand_index(note.hand, staff),
            });

            if note.tie_to_next {
                let x0 = chord.x + chord.head + 0.15;
                let x1 = match chords.get(i + 1) {
                    Some(Some(next)) => next.x - 0.15,
                    _ => width - 0.4,
                };
                push_tie(out, x0, x1, y, down, ink);
            }
            if note.tie_from_prev && i == 0 && options.first_in_half {
                push_tie(out, chord.x - 2.0, chord.x - 0.15, y, down, ink);
            }
        }
    }
    let _ = metrics;
}

fn push_tie(out: &mut Vec<Element>, x0: f32, x1: f32, y: f32, down: bool, ink: Ink) {
    if x1 <= x0 + 0.2 {
        return;
    }
    let height = (0.25 + 0.06 * (x1 - x0)).min(0.8);
    out.push(Element::Tie {
        x0,
        x1,
        y,
        height: if down { height } else { -height },
        ink,
    });
}

fn tuplets(
    out: &mut Vec<Element>,
    voice: &Voice,
    staff: usize,
    chords: &[Option<Chord>],
    metrics: &Metrics,
) {
    for tuplet in &voice.tuplets {
        let members: Vec<&Chord> = (tuplet.first..=tuplet.last)
            .filter_map(|i| chords[i].as_ref())
            .collect();
        let Some(first) = members.first() else {
            continue;
        };
        let last = members.last().unwrap();
        let up = first.up;
        let beamed = voice
            .beams
            .iter()
            .any(|b| b.first <= tuplet.first && tuplet.last <= b.last);

        let x0 = first.x;
        let x1 = last.x + last.head;
        let mid = (x0 + x1) / 2.0;

        // Above the stems for stems up, below them for stems down
        let y = if up {
            members
                .iter()
                .map(|c| c.tip.min(c.outer))
                .fold(f32::MAX, f32::min)
                - 0.9
        } else {
            members
                .iter()
                .map(|c| c.tip.max(c.outer))
                .fold(f32::MIN, f32::max)
                + 1.6
        };

        let digit_w = 0.9;
        out.push(Element::Glyph {
            c: glyphs::tuplet_digit(tuplet.actual as u32),
            x: mid - digit_w / 2.0,
            y: y + 0.45,
            size: 0.9,
            ink: Ink::Dim,
        });

        if !beamed {
            let line_y = y;
            let hook = if up { 0.6 } else { -0.6 };
            for (a, b) in [(x0, mid - digit_w), (mid + digit_w, x1)] {
                if b > a {
                    out.push(Element::Rect {
                        x: a,
                        y: line_y - 0.05,
                        w: b - a,
                        h: 0.1,
                        ink: Ink::Dim,
                    });
                }
            }
            for x in [x0, x1 - 0.1] {
                let (top, h) = if hook > 0.0 {
                    (line_y, hook)
                } else {
                    (line_y + hook, -hook)
                };
                out.push(Element::Rect {
                    x,
                    y: top,
                    w: 0.1,
                    h,
                    ink: Ink::Dim,
                });
            }
        }
    }
    let _ = (staff, metrics);
}

/// 8va / 8vb brackets over runs of events written an octave closer
fn ottava_lines(
    out: &mut Vec<Element>,
    voice: &Voice,
    staff: usize,
    chords: &[Option<Chord>],
    x_of: &dyn Fn(u64) -> f32,
    metrics: &Metrics,
) {
    let events = &voice.events;
    let mut i = 0;
    while i < events.len() {
        if events[i].ottava == 0 || events[i].whole_measure_rest {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < events.len() && events[j + 1].ottava != 0 {
            j += 1;
        }
        let x0 = x_of(events[i].tick) - 0.3;
        let x1 = x_of(events[j].tick) + metrics.head_black + 0.4;

        // Clear of the heads, stems and beams of the run
        let extremes = (i..=j).filter_map(|k| chords[k].as_ref()).map(|c| {
            let head_edge = if staff == 0 {
                c.outer - 0.6
            } else {
                c.outer + 0.6
            };
            if staff == 0 {
                c.tip.min(head_edge)
            } else {
                c.tip.max(head_edge)
            }
        });
        let (glyph, y, hook) = if staff == 0 {
            let top = extremes.fold(staff_top(0), f32::min);
            (glyphs::OTTAVA_ALTA, (top - 1.4).max(PANEL_MARGIN), 0.8)
        } else {
            let bottom = extremes.fold(STAFF_BOTTOM[1], f32::max);
            (
                glyphs::OTTAVA_BASSA,
                (bottom + 1.6).min(PANEL_HEIGHT - PANEL_MARGIN),
                -0.8,
            )
        };

        out.push(Element::Glyph {
            c: glyph,
            x: x0,
            y: y + 0.4,
            size: 0.8,
            ink: Ink::Plain,
        });
        if x1 > x0 + 2.4 {
            out.push(Element::Dashes {
                x0: x0 + 2.2,
                x1,
                y,
                ink: Ink::Plain,
            });
        }
        let (top, h) = if hook > 0.0 {
            (y, hook)
        } else {
            (y + hook, -hook)
        };
        out.push(Element::Rect {
            x: x1,
            y: top,
            w: 0.1,
            h,
            ink: Ink::Plain,
        });
        i = j + 1;
    }
}

impl Chord {
    /// y of the highest and the lowest note head
    fn heads(&self) -> (f32, f32) {
        if self.up {
            (self.outer, self.base + 0.17)
        } else {
            (self.base - 0.17, self.outer)
        }
    }

    /// Where a slur on one side attaches
    fn slur_anchor(&self, above: bool) -> f32 {
        let (top, bottom) = self.heads();
        if above {
            let y = top - 0.9;
            if self.up { y.min(self.tip - 0.4) } else { y }
        } else {
            let y = bottom + 0.9;
            if self.up { y } else { y.max(self.tip + 0.4) }
        }
    }
}

/// Slurs starting, ending or passing through this measure
fn slurs(
    out: &mut Vec<Element>,
    score: &Score,
    index: usize,
    geometry: &[Vec<Vec<Option<Chord>>>; 2],
    width: f32,
) {
    let chord_at = |r: &midi_file::score::EventRef| -> Option<&Chord> {
        geometry.get(r.staff)?.get(r.voice)?.get(r.event)?.as_ref()
    };

    for slur in &score.slurs {
        if slur.start.measure > index || slur.end.measure < index {
            continue;
        }
        let start = (slur.start.measure == index)
            .then(|| chord_at(&slur.start))
            .flatten();
        let end = (slur.end.measure == index)
            .then(|| chord_at(&slur.end))
            .flatten();
        if start.is_none() && end.is_none() && slur.start.measure == index {
            continue;
        }

        // On the head side: below for stems up, above otherwise
        let reference = start.or(end);
        let above = slur
            .above
            .unwrap_or_else(|| reference.is_none_or(|c| !c.up));
        let staff = slur.start.staff;
        let fallback = if above {
            staff_top(staff) - 1.0
        } else {
            STAFF_BOTTOM[staff] + 1.0
        };

        let (x0, y0) = match start {
            Some(c) => (c.x + c.head * 0.6, c.slur_anchor(above)),
            None => (0.3, end.map_or(fallback, |c| c.slur_anchor(above))),
        };
        let (x1, y1) = match end {
            Some(c) => (c.x + c.head * 0.4, c.slur_anchor(above)),
            None => (width - 0.3, y0),
        };
        if x1 <= x0 + 0.5 {
            continue;
        }

        // Curve height from the length, raised to clear the notes in between
        let length = x1 - x0;
        let mut height = (0.5 + 0.06 * length).min(2.0);
        if let Some(voice) = geometry.get(staff).and_then(|g| g.get(slur.start.voice)) {
            for c in voice.iter().flatten() {
                if c.x <= x0 || c.x >= x1 {
                    continue;
                }
                let u = (c.x - x0) / length;
                let line = y0 + (y1 - y0) * u;
                let bulge = 4.0 * u * (1.0 - u);
                if bulge < 0.2 {
                    continue;
                }
                let needed = if above {
                    line - c.slur_anchor(true)
                } else {
                    c.slur_anchor(false) - line
                };
                height = height.max(needed / bulge);
            }
        }
        height = height.min(4.0);

        out.push(Element::Slur {
            x0,
            y0,
            x1,
            y1,
            height: if above { -height } else { height },
            ink: Ink::Plain,
        });
    }
}

/// Where hairpins and dynamics of a staff go, above or below it
fn expression_y(staff: usize, above: bool) -> f32 {
    let between = (STAFF_BOTTOM[0] + staff_top(1)) / 2.0;
    match (staff, above) {
        (0, true) => staff_top(0) - 2.2,
        (0, false) | (1, true) => between,
        _ => STAFF_BOTTOM[1] + 2.6,
    }
}

/// Crescendo / diminuendo hairpins over this measure
fn wedges(out: &mut Vec<Element>, score: &Score, index: usize, ticks: &[(u64, f32)], width: f32) {
    let measure = &score.measures[index];
    for wedge in &score.wedges {
        if wedge.start.0 > index || wedge.end.0 < index {
            continue;
        }
        let (ts, te) = (wedge.start.1, wedge.end.1.max(wedge.start.1 + 1));
        let t0 = ts.max(measure.start_tick);
        let t1 = te.min(measure.end_tick);
        let x0 = if wedge.start.0 == index {
            tick_to_x(ticks, t0)
        } else {
            0.3
        };
        let x1 = if wedge.end.0 == index {
            tick_to_x(ticks, t1)
        } else {
            width - 0.3
        };
        if x1 <= x0 + 0.3 {
            continue;
        }

        let opening = |t: u64| {
            let p = (t.saturating_sub(ts)) as f32 / (te - ts) as f32;
            let p = p.clamp(0.0, 1.0);
            0.9 * if wedge.crescendo { p } else { 1.0 - p }
        };
        let (o0, o1) = (opening(t0), opening(t1));
        let y = expression_y(wedge.staff, wedge.above);
        for sign in [-1.0f32, 1.0] {
            out.push(Element::Beam {
                x0,
                y0: y + sign * o0 / 2.0 - 0.05,
                x1,
                y1: y + sign * o1 / 2.0 - 0.05,
                thickness: 0.1,
                ink: Ink::Plain,
            });
        }
    }
}

/// Dynamics and words, moved aside when they would overlap
fn directions(out: &mut Vec<Element>, measure: &Measure, ticks: &[(u64, f32)]) {
    let mut placed: Vec<(f32, f32, f32)> = Vec::new();
    let mut place = |y: f32, x0: f32, x1: f32, away: f32| -> f32 {
        let mut y = y;
        while placed
            .iter()
            .any(|&(py, a, b)| (py - y).abs() < 1.2 && x0 < b && a < x1)
        {
            y += away;
        }
        placed.push((y, x0, x1));
        y
    };

    for d in &measure.directions {
        let x = tick_to_x(ticks, d.tick);
        // Rows above a staff grow upwards, the others downwards
        let away = if d.staff == 0 && d.above { -1.4 } else { 1.4 };
        match &d.kind {
            midi_file::score::DirectionKind::Dynamic(name) => {
                let letters: Vec<char> = name.chars().filter_map(glyphs::dynamic_letter).collect();
                if letters.is_empty() {
                    continue;
                }
                let w = letters.len() as f32 * 1.1;
                let base = expression_y(d.staff, d.above) + 0.6;
                let y = place(base, x - 0.4, x - 0.4 + w, away);
                for (i, c) in letters.into_iter().enumerate() {
                    out.push(Element::Glyph {
                        c,
                        x: x - 0.4 + i as f32 * 1.05,
                        y,
                        size: 1.0,
                        ink: Ink::Plain,
                    });
                }
            }
            midi_file::score::DirectionKind::Words { text, italic, bold } => {
                let size = 1.3;
                let w = text.chars().count() as f32 * size * 0.5;
                let base = if d.staff == 0 && d.above {
                    staff_top(0) - 2.6
                } else {
                    expression_y(d.staff, d.above) + 0.5
                };
                let y = place(base, x, x + w, away);
                out.push(Element::Text {
                    text: text.clone(),
                    x,
                    y,
                    size,
                    italic: *italic,
                    bold: *bold,
                    ink: Ink::Plain,
                });
            }
        }
    }
}

/// Repeat barline with dots: at the start (thick, thin, dots) or the end (dots, thin, thick)
fn repeat_sign(out: &mut Vec<Element>, x: f32, start: bool) {
    let top = staff_top(0);
    let bottom = STAFF_BOTTOM[1];
    let (thick, thin, dots) = if start {
        (x, x + 0.75, x + 1.15)
    } else {
        (x - 0.5, x - 1.0, x - 1.6)
    };
    out.push(Element::Rect {
        x: thick,
        y: top,
        w: 0.5,
        h: bottom - top,
        ink: Ink::Plain,
    });
    out.push(Element::Rect {
        x: thin,
        y: top,
        w: BARLINE_WIDTH,
        h: bottom - top,
        ink: Ink::Plain,
    });
    for staff in 0..2 {
        for pos in [3, 5] {
            out.push(Element::Glyph {
                c: glyphs::REPEAT_DOT,
                x: dots,
                y: pos_y(staff, pos),
                size: 1.0,
                ink: Ink::Plain,
            });
        }
    }
}

fn tick_to_x(ticks: &[(u64, f32)], tick: u64) -> f32 {
    let i = ticks.partition_point(|(t, _)| *t <= tick);
    if i == 0 {
        return ticks[0].1;
    }
    if i >= ticks.len() {
        return ticks[ticks.len() - 1].1;
    }
    let (ta, xa) = ticks[i - 1];
    let (tb, xb) = ticks[i];
    if tb == ta {
        return xa;
    }
    xa + (xb - xa) * (tick - ta) as f32 / (tb - ta) as f32
}

/// Map a (fractional) tick to x inside an engraved measure
pub fn x_at(engraved: &Engraved, tick: f64) -> f32 {
    let ticks = &engraved.ticks;
    let i = ticks.partition_point(|(t, _)| (*t as f64) <= tick);
    if i == 0 {
        return ticks[0].1;
    }
    if i >= ticks.len() {
        return ticks[ticks.len() - 1].1;
    }
    let (ta, xa) = ticks[i - 1];
    let (tb, xb) = ticks[i];
    if tb == ta {
        return xa;
    }
    xa + (xb - xa) * ((tick - ta as f64) / (tb - ta) as f64) as f32
}

fn pedal(out: &mut Vec<Element>, measure: &Measure, ticks: &[(u64, f32)], width: f32, y: f32) {
    for span in &measure.pedal {
        let x0 = tick_to_x(ticks, span.start);
        let x1 = if span.end >= measure.end_tick {
            width
        } else {
            tick_to_x(ticks, span.end)
        };
        out.push(Element::Rect {
            x: x0,
            y,
            w: (x1 - x0).max(0.1),
            h: 0.12,
            ink: Ink::Dim,
        });
        if !span.continued {
            out.push(Element::Rect {
                x: x0,
                y: y - 0.9,
                w: 0.12,
                h: 0.9 + 0.12,
                ink: Ink::Dim,
            });
        }
        if !span.continues {
            out.push(Element::Rect {
                x: x1 - 0.12,
                y: y - 0.9,
                w: 0.12,
                h: 0.9 + 0.12,
                ink: Ink::Dim,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accidentals_stack_when_close() {
        // A third apart: two columns; an octave apart: one column is enough
        assert_eq!(
            accidental_columns(&[(4, glyphs::SHARP), (6, glyphs::SHARP)]),
            vec![1, 0]
        );
        assert_eq!(
            accidental_columns(&[(0, glyphs::FLAT), (7, glyphs::FLAT)]),
            vec![0, 0]
        );
    }

    #[test]
    fn stem_direction_follows_the_farthest_note() {
        assert!(stem_up_for(&[0, 2]));
        assert!(!stem_up_for(&[6, 8]));
        // Middle line goes down
        assert!(!stem_up_for(&[4]));
        // Chord: the note farther from the middle line decides
        assert!(!stem_up_for(&[3, 10]));
    }

    #[test]
    fn key_change_cancels_old_accidentals() {
        // 3 sharps to 1 sharp: 2 naturals; sharps to flats: all sharps cancelled
        assert_eq!(cancelled_count(3, 1), 2);
        assert_eq!(cancelled_count(2, -2), 2);
        assert_eq!(cancelled_count(0, 4), 0);

        let metrics = Metrics::default();
        let mut out = Vec::new();
        key_signature(
            &mut out,
            0.0,
            -2,
            Some(1),
            [Clef::Treble, Clef::Bass],
            &metrics,
        );
        let naturals = out
            .iter()
            .filter(|e| matches!(e, Element::Glyph { c, .. } if *c == glyphs::NATURAL))
            .count();
        let flats = out
            .iter()
            .filter(|e| matches!(e, Element::Glyph { c, .. } if *c == glyphs::FLAT))
            .count();
        // One natural and two flats, on both staves
        assert_eq!((naturals, flats), (2, 4));
    }

    #[test]
    fn ticks_map_to_x_linearly_between_columns() {
        let engraved = Engraved {
            elements: Vec::new(),
            ticks: vec![(0, 0.0), (480, 10.0), (960, 30.0)],
        };
        assert_eq!(x_at(&engraved, 240.0), 5.0);
        assert_eq!(x_at(&engraved, 720.0), 20.0);
        assert_eq!(x_at(&engraved, 2000.0), 30.0);
    }

    #[test]
    fn beam_ink_spans_the_group() {
        let a = NoteRef {
            start: Duration::from_secs(1),
            end: Duration::from_secs(2),
            hand: 0,
        };
        let b = NoteRef {
            start: Duration::from_secs(3),
            end: Duration::from_secs(4),
            hand: 0,
        };
        match group_ink(Ink::Note(a), Ink::Note(b)) {
            Ink::Note(n) => assert_eq!((n.start, n.end), (a.start, b.end)),
            other => panic!("{other:?}"),
        }
    }
}
