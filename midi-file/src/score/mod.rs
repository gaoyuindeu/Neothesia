//! Staff notation built from a score-like MIDI file (one where notes are aligned to beats).
//!
//! The pipeline:
//! 1. `analysis`: assign notes to the treble/bass staff, merge rolled chords, quantize
//!    every beat to a binary or triplet grid, work out articulation and split each staff
//!    into up to two voices.
//! 2. `notation`: cut the voices into measures, spell notes, split durations into
//!    notatable values joined by ties, group beams and tuplets, mark 8va/8vb and pedal.

mod analysis;
mod notation;

use std::time::Duration;

use crate::{Hand, MidiFile};

pub use notation::spell;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NoteValue {
    ThirtySecond,
    Sixteenth,
    Eighth,
    Quarter,
    Half,
    Whole,
}

impl NoteValue {
    pub const ALL: [NoteValue; 6] = [
        NoteValue::Whole,
        NoteValue::Half,
        NoteValue::Quarter,
        NoteValue::Eighth,
        NoteValue::Sixteenth,
        NoteValue::ThirtySecond,
    ];

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

    /// Length in ticks, with `dots` augmentation dots (each adds half of the previous)
    pub fn ticks(self, ppq: u16, dots: u8) -> u64 {
        let base = self.thirty_seconds() * ppq as u64 / 8;
        let mut total = base;
        let mut add = base;
        for _ in 0..dots {
            add /= 2;
            total += add;
        }
        total
    }

    pub fn has_stem(self) -> bool {
        self != NoteValue::Whole
    }

    /// Number of flags or beams
    pub fn flags(self) -> u8 {
        match self {
            NoteValue::Eighth => 1,
            NoteValue::Sixteenth => 2,
            NoteValue::ThirtySecond => 3,
            _ => 0,
        }
    }

    /// Value (and dots) that lasts exactly `ticks`
    pub fn from_ticks(ticks: u64, ppq: u16) -> Option<(NoteValue, u8)> {
        for value in NoteValue::ALL {
            for dots in 0..=1 {
                let t = value.ticks(ppq, dots);
                // Dotted values must stay exact
                if t == ticks && (dots == 0 || value.ticks(ppq, 0) % (1 << dots) == 0) {
                    return Some((value, dots));
                }
            }
        }
        None
    }

    /// Value closest to `ticks`, for lengths nothing fits exactly
    pub fn closest(ticks: u64, ppq: u16) -> (NoteValue, u8) {
        let mut best = (NoteValue::Quarter, 0, u64::MAX);
        for value in NoteValue::ALL {
            for dots in 0..=1 {
                let diff = value.ticks(ppq, dots).abs_diff(ticks);
                if diff < best.2 {
                    best = (value, dots, diff);
                }
            }
        }
        (best.0, best.1)
    }
}

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
    /// -1 flat, 0 natural, +1 sharp (as spelled)
    pub alter: i8,
    pub accidental: Option<Accidental>,
    /// Playback time of the MIDI note this was written from
    pub start: Duration,
    pub end: Duration,
    pub hand: Option<Hand>,
    /// Continues a note from the previous event (drawn with a tie from there)
    pub tie_from_prev: bool,
    /// Tied to the same pitch in the next event of this voice
    pub tie_to_next: bool,
    /// Trilled (the ornament is shown on the first event of the note)
    pub trill: bool,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub tick: u64,
    /// Real length in ticks
    pub ticks: u64,
    pub value: NoteValue,
    pub dots: u8,
    /// Index into `Voice::tuplets`
    pub tuplet: Option<usize>,
    /// Sorted from the lowest pitch; empty for a rest
    pub notes: Vec<ScoreNote>,
    /// The voice is silent for the whole measure (drawn centered)
    pub whole_measure_rest: bool,
    /// +1: written an octave lower under 8va, -1: an octave higher under 8vb
    pub ottava: i8,
    pub staccato: bool,
    /// Grace notes written small before this chord
    pub grace: Vec<ScoreNote>,
}

impl Event {
    pub fn is_rest(&self) -> bool {
        self.notes.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct Tuplet {
    /// e.g. 3 notes in the time of 2
    pub actual: u8,
    pub normal: u8,
    /// Event index range (inclusive) in the voice
    pub first: usize,
    pub last: usize,
}

#[derive(Debug, Clone)]
pub struct Beam {
    /// Event index range (inclusive) in the voice
    pub first: usize,
    pub last: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Voice {
    pub events: Vec<Event>,
    pub tuplets: Vec<Tuplet>,
    pub beams: Vec<Beam>,
}

#[derive(Debug, Clone, Default)]
pub struct StaffMeasure {
    /// One voice, or two when the staff has independent parts (upper voice first)
    pub voices: Vec<Voice>,
}

/// Sustain pedal held from `start` to `end` (ticks), clipped to a measure
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PedalSpan {
    pub start: u64,
    pub end: u64,
    /// Pressed before this measure
    pub continued: bool,
    /// Released after this measure
    pub continues: bool,
}

#[derive(Debug, Clone)]
pub struct Measure {
    pub index: usize,
    pub start_tick: u64,
    pub end_tick: u64,
    pub start: Duration,
    pub end: Duration,
    /// (numerator, denominator)
    pub time_signature: (u8, u8),
    /// Positive: sharps, negative: flats
    pub key: i8,
    /// Set when the key changes at this measure: the previous key
    pub previous_key: Option<i8>,
    pub time_signature_changed: bool,
    /// Treble and bass staff
    pub staves: [StaffMeasure; 2],
    pub pedal: Vec<PedalSpan>,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub ppq: u16,
    pub measures: Vec<Measure>,
    /// Fraction of note onsets that sit on a 16th / triplet grid.
    /// Performance recordings score low and should not be shown as notation.
    pub grid_alignment: f32,
}

/// Minimum `grid_alignment` for the notation to be worth showing
pub const MIN_GRID_ALIGNMENT: f32 = 0.65;

impl Score {
    pub fn new(file: &MidiFile) -> Self {
        let ppq = file.ppq.max(1);
        let (mut notes, graces, grid_alignment) = analysis::collect(file, ppq);
        let last_tick = notes.iter().map(|n| n.raw_end).max().unwrap_or(0);
        let meters = analysis::meters(file, ppq, last_tick);

        let tuplet_beats = analysis::quantize(&mut notes, &meters, ppq);

        let voices: [[Vec<analysis::Span>; 2]; 2] =
            [0, 1].map(|staff| analysis::voices(&notes, staff, ppq));

        let pedal = analysis::pedal(file, ppq);

        let mut measures =
            notation::build(file, ppq, &meters, &notes, &voices, &tuplet_beats, &pedal);
        notation::attach_graces(&mut measures, &notes, &graces);

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
