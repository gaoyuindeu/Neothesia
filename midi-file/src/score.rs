//! Staff notation built from a score-like MIDI file (one where notes are aligned to beats):
//! measures, chords, rests, note values and accidentals, split into treble and bass staff.

use std::{collections::HashMap, time::Duration};

use crate::{Hand, MidiFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaffKind {
    Treble,
    Bass,
}

impl StaffKind {
    /// Diatonic step of the bottom staff line (E4 for treble, G2 for bass)
    pub fn bottom_line_step(self) -> i32 {
        match self {
            StaffKind::Treble => 4 * 7 + 2,
            StaffKind::Bass => 2 * 7 + 4,
        }
    }
}

pub const STAVES: [StaffKind; 2] = [StaffKind::Treble, StaffKind::Bass];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoteValue {
    ThirtySecond,
    Sixteenth,
    Eighth,
    Quarter,
    Half,
    Whole,
}

impl NoteValue {
    /// Length in 32nd notes
    fn thirty_seconds(self) -> u64 {
        match self {
            NoteValue::ThirtySecond => 1,
            NoteValue::Sixteenth => 2,
            NoteValue::Eighth => 4,
            NoteValue::Quarter => 8,
            NoteValue::Half => 16,
            NoteValue::Whole => 32,
        }
    }

    pub fn ticks(self, ppq: u16, dotted: bool) -> u64 {
        let base = self.thirty_seconds() * ppq as u64 / 8;
        if dotted { base * 3 / 2 } else { base }
    }

    pub fn has_stem(self) -> bool {
        self != NoteValue::Whole
    }

    /// Number of flags (or beams) on the stem
    pub fn flags(self) -> u8 {
        match self {
            NoteValue::Eighth => 1,
            NoteValue::Sixteenth => 2,
            NoteValue::ThirtySecond => 3,
            _ => 0,
        }
    }
}

/// All values a note or rest can be drawn with, longest first
const VALUES: [(NoteValue, bool); 11] = [
    (NoteValue::Whole, true),
    (NoteValue::Whole, false),
    (NoteValue::Half, true),
    (NoteValue::Half, false),
    (NoteValue::Quarter, true),
    (NoteValue::Quarter, false),
    (NoteValue::Eighth, true),
    (NoteValue::Eighth, false),
    (NoteValue::Sixteenth, true),
    (NoteValue::Sixteenth, false),
    (NoteValue::ThirtySecond, false),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accidental {
    Sharp,
    Flat,
    Natural,
}

#[derive(Debug, Clone)]
pub struct ScoreNote {
    pub pitch: u8,
    /// Diatonic step, C0 = 0, C4 = 28
    pub step: i32,
    pub accidental: Option<Accidental>,
    /// Playback time of the MIDI note this was written from
    pub start: Duration,
    pub end: Duration,
    /// This is the continuation of a note that started in an earlier measure
    pub tied: bool,
    pub hand: Option<Hand>,
}

#[derive(Debug, Clone)]
pub struct Chord {
    pub tick: u64,
    pub value: NoteValue,
    pub dotted: bool,
    /// Sorted from the lowest pitch
    pub notes: Vec<ScoreNote>,
}

#[derive(Debug, Clone)]
pub struct Rest {
    pub tick: u64,
    pub value: NoteValue,
    pub dotted: bool,
    /// The staff is empty for the whole measure
    pub whole_measure: bool,
}

#[derive(Debug, Clone)]
pub enum StaffItem {
    Chord(Chord),
    Rest(Rest),
}

impl StaffItem {
    pub fn tick(&self) -> u64 {
        match self {
            StaffItem::Chord(c) => c.tick,
            StaffItem::Rest(r) => r.tick,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Measure {
    pub start_tick: u64,
    pub end_tick: u64,
    pub start: Duration,
    pub end: Duration,
    /// (numerator, denominator)
    pub time_signature: (u8, u8),
    /// Positive: sharps, negative: flats
    pub key: i8,
    /// Treble and bass staff
    pub staves: [Vec<StaffItem>; 2],
}

#[derive(Debug, Clone)]
pub struct Score {
    pub ppq: u16,
    pub measures: Vec<Measure>,
    /// Fraction of note onsets that sit on the 16th / 8th-triplet grid.
    /// Performance recordings score low and should not be shown as notation.
    pub grid_alignment: f32,
}

/// Minimum `grid_alignment` for the notation to be worth showing
pub const MIN_GRID_ALIGNMENT: f32 = 0.65;

struct QuantNote {
    staff: usize,
    hand: Option<Hand>,
    pitch: u8,
    start: u64,
    end: u64,
    start_time: Duration,
    end_time: Duration,
}

impl Score {
    pub fn new(file: &MidiFile) -> Self {
        let ppq = file.ppq.max(1);
        let sixteenth = (ppq as f64 / 4.0).max(1.0);
        let triplet = (ppq as f64 / 3.0).max(1.0);

        let quantize = |tick: u64| -> u64 {
            let t = tick as f64;
            let a = (t / sixteenth).round() * sixteenth;
            let b = (t / triplet).round() * triplet;
            (if (a - t).abs() <= (b - t).abs() { a } else { b }).round() as u64
        };

        let mut notes = Vec::new();
        let mut on_grid = 0;
        for track in file.tracks.iter() {
            if !track.has_other_than_drums {
                continue;
            }
            for note in track.notes.iter().filter(|n| n.channel != 9) {
                let start = quantize(note.start_tick);
                if start.abs_diff(note.start_tick) <= (ppq as u64 / 32).max(1) {
                    on_grid += 1;
                }
                let mut end = quantize(note.end_tick);
                if end <= start {
                    end = start + sixteenth as u64;
                }

                // Staff follows the hand, except for notes far into the other hand's range,
                // which would otherwise need a pile of ledger lines
                let staff = match track.hand {
                    Some(Hand::Right) if note.note < 53 => 1,
                    Some(Hand::Right) => 0,
                    Some(Hand::Left) if note.note >= 67 => 0,
                    Some(Hand::Left) => 1,
                    None if note.note >= 60 => 0,
                    None => 1,
                };

                notes.push(QuantNote {
                    staff,
                    hand: track.hand,
                    pitch: note.note,
                    start,
                    end,
                    start_time: note.start,
                    end_time: note.end,
                });
            }
        }
        notes.sort_by_key(|n| (n.start, n.pitch));

        let grid_alignment = if notes.is_empty() {
            0.0
        } else {
            on_grid as f32 / notes.len() as f32
        };

        let last_tick = notes.iter().map(|n| n.end).max().unwrap_or(0);
        let mut measures = build_measures(file, ppq, last_tick);

        for (staff, _) in STAVES.iter().enumerate() {
            let staff_notes: Vec<&QuantNote> = notes.iter().filter(|n| n.staff == staff).collect();
            for measure in measures.iter_mut() {
                measure.staves[staff] = build_staff(measure, &staff_notes, ppq);
            }
        }

        Self {
            ppq,
            measures,
            grid_alignment,
        }
    }

    pub fn is_readable(&self) -> bool {
        self.grid_alignment >= MIN_GRID_ALIGNMENT && !self.measures.is_empty()
    }

    /// Index of the measure playing at `time` (song time, without lead-in)
    pub fn measure_at(&self, time: Duration) -> usize {
        match self.measures.binary_search_by(|m| m.start.cmp(&time)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => (i - 1).min(self.measures.len().saturating_sub(1)),
        }
    }

    /// Tick at `time`, interpolated inside its measure
    pub fn time_to_tick(&self, time: Duration) -> f64 {
        let Some(measure) = self.measures.get(self.measure_at(time)) else {
            return 0.0;
        };
        let span = (measure.end - measure.start).as_secs_f64().max(1e-6);
        let t = ((time.as_secs_f64() - measure.start.as_secs_f64()) / span).clamp(0.0, 1.0);
        measure.start_tick as f64 + t * (measure.end_tick - measure.start_tick) as f64
    }
}

fn build_measures(file: &MidiFile, ppq: u16, last_tick: u64) -> Vec<Measure> {
    let mut measures = Vec::new();
    let mut tick = 0u64;
    let mut time_sig = (4u8, 4u8);
    let mut key = 0i8;

    while tick < last_tick {
        for ts in file.time_signatures.iter().filter(|ts| ts.tick <= tick) {
            time_sig = (ts.numerator.max(1), ts.denominator.max(1));
        }
        for ks in file.key_signatures.iter().filter(|ks| ks.tick <= tick) {
            key = ks.sharps.clamp(-7, 7);
        }

        let len = (time_sig.0 as u64 * 4 * ppq as u64 / time_sig.1 as u64).max(1);
        let end = tick + len;
        measures.push(Measure {
            start_tick: tick,
            end_tick: end,
            start: file.tempo_track.pulses_to_duration(tick),
            end: file.tempo_track.pulses_to_duration(end),
            time_signature: time_sig,
            key,
            staves: [Vec::new(), Vec::new()],
        });
        tick = end;
    }

    measures
}

/// Largest value (with optional dot) that fits in `ticks`
fn largest_fitting(ticks: u64, ppq: u16) -> Option<(NoteValue, bool)> {
    VALUES
        .iter()
        .copied()
        .find(|(value, dotted)| value.ticks(ppq, *dotted) <= ticks)
}

/// Value closest to `ticks` (used for notes, so triplets still get a sensible look)
fn closest_value(ticks: u64, ppq: u16) -> (NoteValue, bool) {
    VALUES
        .iter()
        .copied()
        .min_by_key(|(value, dotted)| value.ticks(ppq, *dotted).abs_diff(ticks))
        .unwrap()
}

fn fill_rests(from: u64, to: u64, ppq: u16, out: &mut Vec<StaffItem>) {
    let min = NoteValue::ThirtySecond.ticks(ppq, false).max(1);
    let mut tick = from;
    while to > tick && to - tick >= min {
        let Some((value, dotted)) = largest_fitting(to - tick, ppq) else {
            break;
        };
        out.push(StaffItem::Rest(Rest {
            tick,
            value,
            dotted,
            whole_measure: false,
        }));
        tick += value.ticks(ppq, dotted);
    }
}

fn build_staff(measure: &Measure, notes: &[&QuantNote], ppq: u16) -> Vec<StaffItem> {
    // Notes sounding in this measure, clipped to it
    let mut segments: Vec<(u64, u64, &QuantNote)> = notes
        .iter()
        .filter(|n| n.start < measure.end_tick && n.end > measure.start_tick)
        .map(|n| {
            (
                n.start.max(measure.start_tick),
                n.end.min(measure.end_tick),
                *n,
            )
        })
        .collect();
    segments.sort_by_key(|(start, _, n)| (*start, n.pitch));

    if segments.is_empty() {
        return vec![StaffItem::Rest(Rest {
            tick: measure.start_tick,
            value: NoteValue::Whole,
            dotted: false,
            whole_measure: true,
        })];
    }

    let mut onsets: Vec<u64> = segments.iter().map(|(start, _, _)| *start).collect();
    onsets.dedup();

    let mut spelling = Spelling::new(measure.key);
    let mut items = Vec::new();
    let mut cursor = measure.start_tick;

    for (i, &onset) in onsets.iter().enumerate() {
        if onset < cursor {
            // Overlaps the previous chord (a second voice); draw it anyway
        } else {
            fill_rests(cursor, onset, ppq, &mut items);
        }

        let chord_notes: Vec<&(u64, u64, &QuantNote)> =
            segments.iter().filter(|(s, _, _)| *s == onset).collect();

        let longest = chord_notes.iter().map(|(_, e, _)| *e).max().unwrap();
        let next_onset = onsets.get(i + 1).copied().unwrap_or(measure.end_tick);
        // Short gaps are articulation (staccato, detached playing), not written rests
        let gap = next_onset.saturating_sub(longest);
        let written_end = if gap > 0 && (gap < longest - onset || gap <= ppq as u64 / 4) {
            next_onset
        } else {
            longest
        };
        let end = written_end.min(next_onset).max(onset + 1);
        let (value, dotted) = closest_value(end - onset, ppq);

        let notes = chord_notes
            .iter()
            .map(|(start, _, n)| {
                let (step, alter) = spell(n.pitch, measure.key);
                let accidental = spelling.accidental(step, alter);
                ScoreNote {
                    pitch: n.pitch,
                    step,
                    accidental,
                    start: n.start_time,
                    end: n.end_time,
                    tied: *start > n.start,
                    hand: n.hand,
                }
            })
            .collect();

        items.push(StaffItem::Chord(Chord {
            tick: onset,
            value,
            dotted,
            notes,
        }));
        cursor = cursor.max(end);
    }

    fill_rests(cursor, measure.end_tick, ppq, &mut items);
    items
}

/// (diatonic step, alteration) of a pitch; black keys are spelled with flats in flat keys
pub fn spell(pitch: u8, key: i8) -> (i32, i8) {
    const SHARP_LETTERS: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    const SHARP_ALTERS: [i8; 12] = [0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 1, 0];
    const FLAT_LETTERS: [i32; 12] = [0, 1, 1, 2, 2, 3, 4, 4, 5, 5, 6, 6];
    const FLAT_ALTERS: [i8; 12] = [0, -1, 0, -1, 0, 0, -1, 0, -1, 0, -1, 0];

    let pc = (pitch % 12) as usize;
    let octave = pitch as i32 / 12 - 1;
    let (letter, alter) = if key < 0 {
        (FLAT_LETTERS[pc], FLAT_ALTERS[pc])
    } else {
        (SHARP_LETTERS[pc], SHARP_ALTERS[pc])
    };
    (octave * 7 + letter, alter)
}

/// Tracks which alterations are in effect within a measure
struct Spelling {
    key_alters: [i8; 7],
    in_measure: HashMap<i32, i8>,
}

impl Spelling {
    fn new(key: i8) -> Self {
        // Letters: C=0 D=1 E=2 F=3 G=4 A=5 B=6
        const SHARP_ORDER: [usize; 7] = [3, 0, 4, 1, 5, 2, 6];
        const FLAT_ORDER: [usize; 7] = [6, 2, 5, 1, 4, 0, 3];

        let mut key_alters = [0; 7];
        if key > 0 {
            for &letter in SHARP_ORDER.iter().take(key as usize) {
                key_alters[letter] = 1;
            }
        } else {
            for &letter in FLAT_ORDER.iter().take((-key) as usize) {
                key_alters[letter] = -1;
            }
        }

        Self {
            key_alters,
            in_measure: HashMap::new(),
        }
    }

    fn accidental(&mut self, step: i32, alter: i8) -> Option<Accidental> {
        let letter = step.rem_euclid(7) as usize;
        let current = self
            .in_measure
            .get(&step)
            .copied()
            .unwrap_or(self.key_alters[letter]);
        if current == alter {
            return None;
        }
        self.in_measure.insert(step, alter);
        Some(match alter {
            1 => Accidental::Sharp,
            -1 => Accidental::Flat,
            _ => Accidental::Natural,
        })
    }
}
