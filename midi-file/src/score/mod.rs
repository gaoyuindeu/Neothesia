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

/// Mark very high / low passages as 8va / 8vb automatically
pub fn auto_ottava(measures: &mut [Measure]) {
    notation::ottava(measures);
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Clef {
    #[default]
    Treble,
    Bass,
    /// C clef on the middle line
    Alto,
    /// C clef on the fourth line
    Tenor,
    /// Treble clef sounding an octave lower / higher (8 below / above the clef)
    Treble8vb,
    Treble8va,
    Bass8vb,
}

impl Clef {
    /// Diatonic step (C0 = 0) of the note on the bottom staff line
    pub fn bottom_line_step(self) -> i32 {
        match self {
            Clef::Treble => 4 * 7 + 2,
            Clef::Treble8vb => 3 * 7 + 2,
            Clef::Treble8va => 5 * 7 + 2,
            Clef::Bass => 2 * 7 + 4,
            Clef::Bass8vb => 7 + 4,
            Clef::Alto => 3 * 7 + 3,
            Clef::Tenor => 2 * 7 + 1,
        }
    }

    pub fn for_staff(staff: usize) -> Self {
        if staff == 0 { Clef::Treble } else { Clef::Bass }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Articulation {
    Accent,
    Tenuto,
    Marcato,
    Staccatissimo,
    Fermata,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ornament {
    Trill,
    Turn,
    InvertedTurn,
    Mordent,
    InvertedMordent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum NoteValue {
    SixtyFourth,
    ThirtySecond,
    Sixteenth,
    Eighth,
    #[default]
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

    /// Length in 64th notes
    fn sixty_fourths(self) -> u64 {
        match self {
            NoteValue::SixtyFourth => 1,
            NoteValue::ThirtySecond => 2,
            NoteValue::Sixteenth => 4,
            NoteValue::Eighth => 8,
            NoteValue::Quarter => 16,
            NoteValue::Half => 32,
            NoteValue::Whole => 64,
        }
    }

    /// Length in ticks, with `dots` augmentation dots (each adds half of the previous)
    pub fn ticks(self, ppq: u16, dots: u8) -> u64 {
        let base = self.sixty_fourths() * ppq as u64 / 16;
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
            NoteValue::SixtyFourth => 4,
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
    DoubleSharp,
    DoubleFlat,
}

#[derive(Debug, Clone, Default)]
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
    pub fingering: Option<String>,
}

#[derive(Debug, Clone, Default)]
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
    /// Clef in effect for this event
    pub clef: Clef,
    /// Stem direction given by the score (true: up); computed when None
    pub stem: Option<bool>,
    pub articulations: Vec<Articulation>,
    pub ornament: Option<Ornament>,
    /// Rolled chord (wavy line before it)
    pub arpeggio: bool,
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
    /// Draw a bracket (None: only when not beamed)
    pub bracket: Option<bool>,
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
    /// One voice, or more when the staff has independent parts (upper voice first;
    /// with several voices, even ones have stems up, odd ones down)
    pub voices: Vec<Voice>,
    /// Clef at the start of the measure
    pub clef: Clef,
    /// Clef changes inside the measure: (tick, new clef)
    pub clef_changes: Vec<(u64, Clef)>,
}

/// Marks attached to a point in time
#[derive(Debug, Clone, PartialEq)]
pub enum DirectionKind {
    /// "p", "mf", "sfz", ...
    Dynamic(String),
    Words {
        text: String,
        italic: bool,
        bold: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Direction {
    pub tick: u64,
    pub staff: usize,
    pub above: bool,
    pub kind: DirectionKind,
}

/// Points at an event: measure, staff, voice and event index
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventRef {
    pub measure: usize,
    pub staff: usize,
    pub voice: usize,
    pub event: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slur {
    pub start: EventRef,
    pub end: EventRef,
    /// Curve above the notes (None: decide from the stems)
    pub above: Option<bool>,
}

/// Crescendo / diminuendo hairpin, (measure, tick) at both ends
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wedge {
    pub staff: usize,
    pub start: (usize, u64),
    pub end: (usize, u64),
    pub crescendo: bool,
    pub above: bool,
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
    /// Number shown on the score
    pub number: String,
    pub repeat_start: bool,
    pub repeat_end: bool,
    /// Volta ending label ("1.", "2.") when the measure starts one
    pub ending: Option<String>,
    pub directions: Vec<Direction>,
}

#[derive(Debug, Clone)]
pub struct Score {
    pub ppq: u16,
    pub measures: Vec<Measure>,
    /// Fraction of note onsets that sit on a 16th / triplet grid.
    /// Performance recordings score low and should not be shown as notation.
    pub grid_alignment: f32,
    pub slurs: Vec<Slur>,
    pub wedges: Vec<Wedge>,
}

/// Minimum `grid_alignment` for the notation to be worth showing
pub const MIN_GRID_ALIGNMENT: f32 = 0.65;

impl Score {
    pub fn new(file: &MidiFile) -> Self {
        // Scores read from notation files come with their own notation
        if let Some(score) = &file.score {
            return (**score).clone();
        }
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
            slurs: Vec::new(),
            wedges: Vec::new(),
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
