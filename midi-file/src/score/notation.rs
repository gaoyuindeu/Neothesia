//! From voices to notation: measures, notatable values, ties, tuplets, beams,
//! accidentals, 8va/8vb and pedal.

use std::{cmp::Reverse, collections::HashMap};

use super::{
    Accidental, Beam, Clef, Event, Measure, NoteValue, PedalSpan, STAVES, ScoreNote, StaffMeasure,
    Tuplet, Voice,
    analysis::{Grace, Meter, Note, Span, TupletBeat},
};
use crate::MidiFile;

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

/// Alterations in effect within a measure
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

    fn accidental(&mut self, step: i32, alter: i8, tied: bool) -> Option<Accidental> {
        let letter = step.rem_euclid(7) as usize;
        let current = self
            .in_measure
            .get(&step)
            .copied()
            .unwrap_or(self.key_alters[letter]);
        self.in_measure.insert(step, alter);
        if current == alter || tied {
            return None;
        }
        Some(match alter {
            1 => Accidental::Sharp,
            -1 => Accidental::Flat,
            _ => Accidental::Natural,
        })
    }
}

/// A piece of a duration that can be written as one note or rest
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Piece {
    pub start: u64,
    pub len: u64,
    pub value: NoteValue,
    pub dots: u8,
    /// Start of the tuplet beat this piece belongs to
    pub tuplet: Option<u64>,
}

/// Whether [x, y) can be one note without hiding the meter
fn fits(x: u64, y: u64, m: &Meter, ppq: u16) -> Option<(NoteValue, u8)> {
    let (value, dots) = NoteValue::from_ticks(y - x, ppq)?;
    let r = x - m.start;
    let within_beat = r / m.beat == (y - m.start - 1) / m.beat;
    if within_beat {
        return Some((value, dots));
    }
    // Longer than a beat: must start on a beat and not cross the middle of the bar
    if r % m.beat != 0 {
        return None;
    }
    if let Some(half) = m.half_bar()
        && r != 0
        && r < half
        && y - m.start > half
    {
        return None;
    }
    Some((value, dots))
}

fn binary_pieces(x: u64, y: u64, m: &Meter, ppq: u16, out: &mut Vec<Piece>) {
    if y <= x {
        return;
    }
    if let Some((value, dots)) = fits(x, y, m, ppq) {
        out.push(Piece {
            start: x,
            len: y - x,
            value,
            dots,
            tuplet: None,
        });
        return;
    }

    // Split at the metrically strongest point inside
    let step = (ppq as u64 / 16).max(1);
    let first = m.start + ((x - m.start) / step + 1) * step;
    let split = (first..y)
        .step_by(step as usize)
        .max_by_key(|&t| (m.strength(t), Reverse(t)));

    match split {
        Some(p) => {
            binary_pieces(x, p, m, ppq, out);
            binary_pieces(p, y, m, ppq, out);
        }
        None => {
            let (value, dots) = NoteValue::closest(y - x, ppq);
            out.push(Piece {
                start: x,
                len: y - x,
                value,
                dots,
                tuplet: None,
            });
        }
    }
}

fn tuplet_pieces(x: u64, y: u64, beat: &TupletBeat, ppq: u16, out: &mut Vec<Piece>) {
    if y <= x {
        return;
    }
    let len = y - x;
    // 3 in the time of 2: written values are 3/2 of the real length
    if (len * 3) % 2 == 0
        && let Some((value, dots)) = NoteValue::from_ticks(len * 3 / 2, ppq)
    {
        out.push(Piece {
            start: x,
            len,
            value,
            dots,
            tuplet: Some(beat.start),
        });
        return;
    }

    let grid = (beat.len / beat.divisions).max(1);
    let p = beat.start + ((x - beat.start) / grid + 1) * grid;
    if p < y {
        tuplet_pieces(x, p, beat, ppq, out);
        tuplet_pieces(p, y, beat, ppq, out);
    } else {
        let (value, dots) = NoteValue::closest(len * 3 / 2, ppq);
        out.push(Piece {
            start: x,
            len,
            value,
            dots,
            tuplet: Some(beat.start),
        });
    }
}

/// Split [a, b) inside one measure into notatable pieces
pub(super) fn decompose(a: u64, b: u64, m: &Meter, ppq: u16, tuplets: &[TupletBeat]) -> Vec<Piece> {
    let mut cuts = vec![a, b];
    for t in tuplets
        .iter()
        .filter(|t| t.start < b && t.start + t.len > a)
    {
        for c in [t.start, t.start + t.len] {
            if c > a && c < b {
                cuts.push(c);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();

    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let (x, y) = (w[0], w[1]);
        match tuplets
            .iter()
            .find(|t| t.start <= x && y <= t.start + t.len)
        {
            Some(beat) => tuplet_pieces(x, y, beat, ppq, &mut out),
            None => binary_pieces(x, y, m, ppq, &mut out),
        }
    }
    out
}

/// A span clipped to a measure, or a gap between spans
struct Chunk<'a> {
    start: u64,
    end: u64,
    span: Option<&'a Span>,
    tie_from_prev: bool,
    tie_to_next: bool,
}

fn build_voice(
    spans: &[Span],
    m: &Meter,
    notes: &[Note],
    ppq: u16,
    tuplet_beats: &[TupletBeat],
    primary: bool,
    clef: Clef,
) -> Option<Voice> {
    let (ms, me) = (m.start, m.end());

    let first = spans.partition_point(|s| s.end <= ms);
    let clipped: Vec<&Span> = spans[first..]
        .iter()
        .take_while(|s| s.start < me)
        .filter(|s| s.end > ms)
        .collect();

    if clipped.is_empty() {
        if !primary {
            return None;
        }
        return Some(Voice {
            events: vec![Event {
                tick: ms,
                ticks: m.len,
                value: NoteValue::Whole,
                dots: 0,
                tuplet: None,
                notes: Vec::new(),
                whole_measure_rest: true,
                clef,
                ..Default::default()
            }],
            ..Default::default()
        });
    }

    let mut chunks = Vec::new();
    let mut cursor = ms;
    for span in clipped {
        let start = span.start.max(ms).max(cursor);
        let end = span.end.min(me);
        if end <= start {
            continue;
        }
        if start > cursor {
            chunks.push(Chunk {
                start: cursor,
                end: start,
                span: None,
                tie_from_prev: false,
                tie_to_next: false,
            });
        }
        chunks.push(Chunk {
            start,
            end,
            span: Some(span),
            tie_from_prev: span.start < ms,
            tie_to_next: span.end > me,
        });
        cursor = end;
    }
    if cursor < me {
        chunks.push(Chunk {
            start: cursor,
            end: me,
            span: None,
            tie_from_prev: false,
            tie_to_next: false,
        });
    }

    let local_tuplets: Vec<TupletBeat> = tuplet_beats
        .iter()
        .filter(|t| t.start >= ms && t.start < me)
        .copied()
        .collect();

    let mut events = Vec::new();
    let mut tuplet_of_event: Vec<Option<u64>> = Vec::new();
    for chunk in chunks {
        // Rests of a second voice only add clutter in piano music
        if !primary && chunk.span.is_none() {
            continue;
        }
        let pieces = decompose(chunk.start, chunk.end, m, ppq, &local_tuplets);
        let count = pieces.len();
        for (i, piece) in pieces.into_iter().enumerate() {
            let notes: Vec<ScoreNote> = chunk
                .span
                .map(|span| {
                    span.notes
                        .iter()
                        .map(|&n| {
                            let note = &notes[n];
                            let (step, alter) = spell(note.pitch, m.key);
                            ScoreNote {
                                pitch: note.pitch,
                                step,
                                alter,
                                accidental: None,
                                start: note.start_time,
                                end: note.end_time,
                                hand: note.hand,
                                tie_from_prev: i > 0 || chunk.tie_from_prev,
                                tie_to_next: i + 1 < count || chunk.tie_to_next,
                                trill: note.trill && i == 0 && !chunk.tie_from_prev,
                                ..Default::default()
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            let staccato = chunk
                .span
                .is_some_and(|s| s.staccato && i == 0 && !chunk.tie_from_prev);

            events.push(Event {
                tick: piece.start,
                ticks: piece.len,
                value: piece.value,
                dots: piece.dots,
                tuplet: None,
                notes,
                staccato,
                clef,
                ..Default::default()
            });
            tuplet_of_event.push(piece.tuplet);
        }
    }

    // Tuplets: runs of events in the same tuplet beat
    let mut tuplets = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if let Some(beat) = tuplet_of_event[i] {
            let mut j = i;
            while j + 1 < events.len() && tuplet_of_event[j + 1] == Some(beat) {
                j += 1;
            }
            let index = tuplets.len();
            let sextuplet = local_tuplets
                .iter()
                .any(|t| t.start == beat && t.divisions >= 6)
                && j - i + 1 > 3;
            tuplets.push(Tuplet {
                actual: if sextuplet { 6 } else { 3 },
                normal: if sextuplet { 4 } else { 2 },
                bracket: None,
                first: i,
                last: j,
            });
            for event in &mut events[i..=j] {
                event.tuplet = Some(index);
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }

    let beams = beams(&events, &tuplet_of_event, m, ppq);

    Some(Voice {
        events,
        tuplets,
        beams,
    })
}

/// Group flagged notes into beams, by beat (by tuplet for tuplets); eighths in
/// 2/4 and 4/4 may share a beam across the two beats of a half bar
fn beams(events: &[Event], tuplet_of_event: &[Option<u64>], m: &Meter, ppq: u16) -> Vec<Beam> {
    let beamable = |e: &Event| !e.is_rest() && e.value.flags() > 0;
    let unit = |i: usize| -> (bool, u64) {
        match tuplet_of_event[i] {
            Some(start) => (true, start),
            None => (false, (events[i].tick - m.start) / m.beat),
        }
    };

    let mut groups: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < events.len() {
        if !beamable(&events[i]) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j + 1 < events.len()
            && beamable(&events[j + 1])
            && unit(j + 1) == unit(i)
            && events[j].tick + events[j].ticks == events[j + 1].tick
        {
            j += 1;
        }
        groups.push((i, j));
        i = j + 1;
    }

    // Eighths across the two beats of a half bar
    let pairs_by_half = !m.compound && m.beat == ppq as u64 && m.beats() % 2 == 0;
    let plain_eighths = |(a, b): (usize, usize)| {
        events[a..=b]
            .iter()
            .all(|e| e.value == NoteValue::Eighth && e.dots == 0 && e.tuplet.is_none())
    };
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for group in groups {
        if pairs_by_half
            && let Some(last) = merged.last_mut()
            && plain_eighths(*last)
            && plain_eighths(group)
            && last.1 + 1 == group.0
        {
            let beat_a = (events[last.0].tick - m.start) / m.beat;
            let beat_b = (events[group.0].tick - m.start) / m.beat;
            if beat_b == beat_a + 1 && beat_a % 2 == 0 {
                last.1 = group.1;
                continue;
            }
        }
        merged.push(group);
    }

    merged
        .into_iter()
        .filter(|(a, b)| b > a)
        .map(|(first, last)| Beam { first, last })
        .collect()
}

fn accidentals(staff: &mut StaffMeasure, key: i8) {
    let mut order: Vec<(u64, usize, usize)> = Vec::new();
    for (v, voice) in staff.voices.iter().enumerate() {
        for (e, event) in voice.events.iter().enumerate() {
            if !event.is_rest() {
                order.push((event.tick, v, e));
            }
        }
    }
    order.sort_unstable();

    let mut spelling = Spelling::new(key);
    for (_, v, e) in order {
        for note in staff.voices[v].events[e].notes.iter_mut() {
            note.accidental = spelling.accidental(note.step, note.alter, note.tie_from_prev);
        }
    }
}

/// Write very high treble (very low bass) passages an octave closer, marked 8va/8vb.
/// Decided per beamed group, so a line doesn't flicker on and off note by note.
fn ottava(measures: &mut [Measure]) {
    for measure in measures.iter_mut() {
        for (staff, kind) in STAVES.iter().enumerate() {
            let bottom = kind.bottom_line_step();
            for voice in measure.staves[staff].voices.iter_mut() {
                let mut segments: Vec<(usize, usize)> = Vec::new();
                let mut i = 0;
                while i < voice.events.len() {
                    let end = voice
                        .beams
                        .iter()
                        .find(|b| b.first == i)
                        .map_or(i, |b| b.last);
                    segments.push((i, end));
                    i = end + 1;
                }

                let mut flags = vec![0i8; voice.events.len()];
                for &(a, b) in &segments {
                    let positions: Vec<i32> = voice.events[a..=b]
                        .iter()
                        .flat_map(|e| e.notes.iter().map(|n| n.step - bottom))
                        .collect();
                    let (Some(&low), Some(&high)) =
                        (positions.iter().min(), positions.iter().max())
                    else {
                        continue;
                    };
                    let shift = if staff == 0 {
                        // From four ledger lines up; at most three below once shifted down
                        (high >= 16 && low - 7 >= -6).then_some(1)
                    } else {
                        (low <= -8 && high + 7 <= 14).then_some(-1)
                    };
                    if let Some(shift) = shift {
                        flags[a..=b].iter_mut().for_each(|f| *f = shift);
                    }
                }

                // Bridge single notes and rests between two 8va segments when they fit
                for k in 1..flags.len().saturating_sub(1) {
                    if flags[k] == 0 && flags[k - 1] != 0 && flags[k - 1] == flags[k + 1] {
                        let fits = voice.events[k].notes.iter().all(|n| {
                            let p = n.step - bottom - 7 * flags[k - 1] as i32;
                            (-6..=14).contains(&p)
                        });
                        if fits {
                            flags[k] = flags[k - 1];
                        }
                    }
                }

                for (event, flag) in voice.events.iter_mut().zip(flags) {
                    event.ottava = flag;
                }
            }
        }
    }
}

/// A voice whose notes all line up (same start and length) with events of the other
/// voice is just part of its chords. Fold it in when the other voice covers the whole
/// measure, so no rests go missing.
fn merge_voices(staff: &mut StaffMeasure, m: &Meter) {
    if staff.voices.len() != 2 {
        return;
    }
    let covers_measure = |voice: &Voice| {
        let mut t = m.start;
        for e in &voice.events {
            if e.tick != t {
                return false;
            }
            t += e.ticks;
        }
        t == m.end()
    };
    let lines_up = |sparse: &Voice, full: &Voice| {
        sparse.events.iter().filter(|e| !e.is_rest()).all(|e| {
            full.events
                .iter()
                .any(|f| !f.is_rest() && f.tick == e.tick && f.ticks == e.ticks)
        })
    };

    for (sparse, full) in [(1, 0), (0, 1)] {
        if !lines_up(&staff.voices[sparse], &staff.voices[full])
            || !covers_measure(&staff.voices[full])
        {
            continue;
        }
        let from = staff.voices.remove(sparse);
        let target = &mut staff.voices[0];
        for event in from.events.into_iter().filter(|e| !e.is_rest()) {
            let f = target
                .events
                .iter_mut()
                .find(|f| !f.is_rest() && f.tick == event.tick && f.ticks == event.ticks)
                .unwrap();
            f.staccato &= event.staccato;
            f.notes.extend(event.notes);
            f.notes.sort_by_key(|n| n.pitch);
        }
        return;
    }
}

/// Whether the second voice sits higher than the first within this measure
fn upper_is_second(voices: &[Vec<Span>; 2], m: &Meter, notes: &[Note]) -> bool {
    let average = |spans: &Vec<Span>| {
        let pitches: Vec<f32> = spans
            .iter()
            .filter(|s| s.start < m.end() && s.end > m.start)
            .flat_map(|s| s.notes.iter().map(|&n| notes[n].pitch as f32))
            .collect();
        (!pitches.is_empty()).then(|| pitches.iter().sum::<f32>() / pitches.len() as f32)
    };
    matches!((average(&voices[0]), average(&voices[1])), (Some(a), Some(b)) if b > a)
}

pub(super) fn build(
    file: &MidiFile,
    ppq: u16,
    meters: &[Meter],
    notes: &[Note],
    voices: &[[Vec<Span>; 2]; 2],
    tuplet_beats: &[Vec<TupletBeat>; 2],
    pedal: &[(u64, u64)],
) -> Vec<Measure> {
    let mut measures: Vec<Measure> = meters
        .iter()
        .enumerate()
        .map(|(index, m)| {
            let previous = index.checked_sub(1).map(|i| meters[i]);
            let staves = [0, 1].map(|staff| {
                let clef = Clef::for_staff(staff);
                let mut staff_measure = StaffMeasure {
                    clef,
                    ..Default::default()
                };
                // The upper part becomes the first voice (stems up, with rests)
                let order = if upper_is_second(&voices[staff], m, notes) {
                    [1, 0]
                } else {
                    [0, 1]
                };
                for (rank, v) in order.into_iter().enumerate() {
                    if let Some(voice) = build_voice(
                        &voices[staff][v],
                        m,
                        notes,
                        ppq,
                        &tuplet_beats[staff],
                        rank == 0,
                        clef,
                    ) {
                        staff_measure.voices.push(voice);
                    }
                }
                merge_voices(&mut staff_measure, m);
                accidentals(&mut staff_measure, m.key);
                staff_measure
            });

            let pedal = pedal
                .iter()
                .filter(|(a, b)| *a < m.end() && *b > m.start)
                .map(|&(a, b)| PedalSpan {
                    start: a.max(m.start),
                    end: b.min(m.end()),
                    continued: a < m.start,
                    continues: b > m.end(),
                })
                .collect();

            Measure {
                index,
                start_tick: m.start,
                end_tick: m.end(),
                start: file.tempo_track.pulses_to_duration(m.start),
                end: file.tempo_track.pulses_to_duration(m.end()),
                time_signature: (m.num, m.den),
                key: m.key,
                previous_key: previous.filter(|p| p.key != m.key).map(|p| p.key),
                time_signature_changed: previous.is_some_and(|p| (p.num, p.den) != (m.num, m.den)),
                staves,
                pedal,
                number: (index + 1).to_string(),
                repeat_start: false,
                repeat_end: false,
                ending: None,
                directions: Vec::new(),
            }
        })
        .collect();

    ottava(&mut measures);
    measures
}

/// Hang grace notes on the chord of their main note
pub(super) fn attach_graces(measures: &mut [Measure], notes: &[Note], graces: &[Grace]) {
    for grace in graces {
        let Some(main) = notes
            .iter()
            .find(|n| n.staff == grace.staff && (n.raw_start, n.pitch) == grace.main)
        else {
            continue;
        };
        let i = measures.partition_point(|m| m.start_tick <= main.start);
        let Some(measure) = i.checked_sub(1).and_then(|i| measures.get_mut(i)) else {
            continue;
        };
        let key = measure.key;
        let event = measure.staves[grace.staff]
            .voices
            .iter_mut()
            .flat_map(|v| v.events.iter_mut())
            .find(|e| e.tick == main.start && !e.is_rest());
        let Some(event) = event else { continue };

        let (step, alter) = spell(grace.pitch, key);
        let key_alter = Spelling::new(key).key_alters[step.rem_euclid(7) as usize];
        event.grace.push(ScoreNote {
            pitch: grace.pitch,
            step,
            alter,
            accidental: (alter != key_alter).then_some(match alter {
                1 => Accidental::Sharp,
                -1 => Accidental::Flat,
                _ => Accidental::Natural,
            }),
            start: grace.start_time,
            end: grace.end_time,
            hand: grace.hand,
            tie_from_prev: false,
            tie_to_next: false,
            trill: false,
            ..Default::default()
        });
    }
}
