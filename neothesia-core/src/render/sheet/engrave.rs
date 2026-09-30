//! Layout of one measure of the grand staff.
//!
//! Everything is measured in staff spaces: x from the left edge of the measure,
//! y downwards from the top of the sheet panel. The renderer scales it to pixels.

use std::{collections::HashMap, time::Duration};

use midi_file::{
    Hand,
    score::{Accidental, Event, Measure, NoteValue, STAVES, Score, Voice},
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
    /// Draw key/time signature changes at the start of the measure
    pub show_changes: bool,
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
fn positions(event: &Event, staff: usize) -> Vec<i32> {
    let bottom = STAVES[staff].bottom_line_step();
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

/// Key signature on both staves starting at x
pub fn key_signature(
    out: &mut Vec<Element>,
    x: f32,
    key: i8,
    previous: Option<i8>,
    metrics: &Metrics,
) -> f32 {
    // Positions above the treble bottom line; the bass uses the same pattern 2 lower
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
                    y: pos_y(staff, pos - 2 * staff as i32),
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
                y: pos_y(staff, pos - 2 * staff as i32),
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

fn changes_width(measure: &Measure, metrics: &Metrics) -> f32 {
    let mut width = 0.0;
    if let Some(previous) = measure.previous_key {
        width += key_signature_width(measure.key, Some(previous), metrics);
    }
    if measure.time_signature_changed {
        width += time_signature_width(measure.time_signature, metrics);
    }
    width
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

pub fn plan(score: &Score, index: usize, metrics: &Metrics, show_changes: bool) -> MeasurePlan {
    let measure = &score.measures[index];
    let lead = 1.0
        + if show_changes {
            changes_width(measure, metrics)
        } else {
            0.0
        };

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
            if has_second(&positions(event, staff)) {
                left += head;
                right += head;
            }
            if event.value.flags() > 0 && !is_beamed(voice, event) {
                right += metrics.flag * 0.8;
            }
        }
        if event.dots > 0 {
            right += 0.35 + event.dots as f32 * 0.5;
        }
        column.left = column.left.max(left);
        column.right = column.right.max(right);
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
        x + 0.5
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

    // Key / time changes at the start
    if options.show_changes {
        let mut x = 0.6;
        if let Some(previous) = measure.previous_key {
            x += key_signature(&mut out, x, measure.key, Some(previous), metrics);
        }
        if measure.time_signature_changed {
            time_signature(&mut out, x, measure.time_signature, metrics);
        }
    }

    // Measure number
    if options.first_in_half {
        let number = (measure.index + 1).to_string();
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

    for staff in 0..2 {
        let voices = &measure.staves[staff].voices;
        for (v, voice) in voices.iter().enumerate() {
            let forced = (voices.len() == 2).then_some(v == 0);
            engrave_voice(
                &mut out, voice, staff, v, forced, &x_of, width, metrics, options,
            );
        }
    }

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
        })
        .fold(STAFF_BOTTOM[1], f32::max);
    let pedal_y = (lowest + 1.3)
        .max(STAFF_BOTTOM[1] + 2.8)
        .min(PANEL_HEIGHT - 0.4);
    pedal(&mut out, measure, &ticks, width, pedal_y);

    // Barline
    let top = staff_top(0);
    let bottom = STAFF_BOTTOM[1];
    if options.last_measure {
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
    staff: usize,
    v: usize,
    forced_up: Option<bool>,
    x_of: &dyn Fn(u64) -> f32,
    width: f32,
    metrics: &Metrics,
    options: Options,
) {
    // Stem direction: fixed with two voices, per beam group or per chord otherwise
    let mut up: Vec<bool> = voice
        .events
        .iter()
        .map(|e| forced_up.unwrap_or_else(|| stem_up_for(&positions(e, staff))))
        .collect();
    if forced_up.is_none() {
        for beam in &voice.beams {
            let all: Vec<i32> = voice.events[beam.first..=beam.last]
                .iter()
                .flat_map(|e| positions(e, staff))
                .collect();
            let dir = stem_up_for(&all);
            up[beam.first..=beam.last].iter_mut().for_each(|u| *u = dir);
        }
    }

    let mut chords: Vec<Option<Chord>> = Vec::with_capacity(voice.events.len());
    for (i, event) in voice.events.iter().enumerate() {
        if event.is_rest() {
            rest(out, event, staff, forced_up, x_of, width);
            chords.push(None);
        } else {
            let x = x_of(event.tick);
            chords.push(Some(chord(out, event, staff, v, up[i], x, metrics)));
        }
    }

    // Beams set the stem tips of their chords
    for beam in &voice.beams {
        beam_group(out, voice, beam.first, beam.last, &mut chords, staff);
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
    };
    // With two voices, rests move out of the way of the other voice
    match forced_up {
        Some(true) => pos += 4,
        Some(false) => pos -= 4,
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

    let ink = get(members[0]).ink;
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
