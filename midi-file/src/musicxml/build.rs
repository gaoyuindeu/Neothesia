//! Raw MusicXML measures to the score model, plus MIDI for playback.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use midly::{
    Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
    num::{u4, u7, u15, u24, u28},
};

use super::parse::{RawDirectionKind, RawElement, RawNote, RawScore, RawSound};
use crate::{
    Hand, MidiFile,
    score::{
        Accidental, Articulation, Beam, Clef, Direction, DirectionKind, Event, EventRef, Measure,
        NoteValue, Ornament, PedalSpan, Score, ScoreNote, Slur, StaffMeasure, Tuplet, Voice, Wedge,
    },
};

/// Highest tick resolution used; finer files are rounded
const MAX_PPQ: u64 = 7680;
const DEFAULT_TEMPO: f64 = 100.0;

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn lcm(a: u64, b: u64) -> u64 {
    a / gcd(a, b) * b
}

/// A note placed in time (ticks) on the unrolled score
#[derive(Debug, Clone)]
struct Placed {
    measure: usize,
    staff: usize,
    /// (part, voice number)
    voice: (usize, u32),
    start: u64,
    duration: u64,
    note: RawNote,
}

#[derive(Debug, Clone)]
struct PlacedDirection {
    measure: usize,
    staff: usize,
    tick: u64,
    above: Option<bool>,
    kind: RawDirectionKind,
}

#[derive(Debug, Clone, Default)]
struct MeasureInfo {
    start: u64,
    len: u64,
    time: (u8, u8),
    key: i8,
    number: String,
    repeat_start: bool,
    repeat_end: bool,
    ending: Option<String>,
    /// Clef of each staff at the start
    clefs: [Clef; 2],
}

/// Order in which the raw measures are played, repeats and jumps unrolled
fn play_order(raw: &RawScore) -> Vec<usize> {
    let Some(part) = raw.parts.first() else {
        return Vec::new();
    };
    let n = part.measures.len();

    let mut forward = vec![false; n];
    let mut backward: Vec<Option<u32>> = vec![None; n];
    let mut ending_start: Vec<Option<Vec<u32>>> = vec![None; n];
    let mut ending_stop = vec![false; n];
    let mut sounds: Vec<RawSound> = vec![RawSound::default(); n];
    let mut segno = None;
    let mut coda: Vec<usize> = Vec::new();

    for (i, m) in part.measures.iter().enumerate() {
        for element in &m.elements {
            match element {
                RawElement::Barline(b) => {
                    if let Some((direction, times)) = &b.repeat {
                        if direction == "forward" {
                            forward[i] = true;
                        } else {
                            backward[i] = Some((*times).max(2));
                        }
                    }
                    if let Some((numbers, kind)) = &b.ending {
                        if kind == "start" {
                            ending_start[i] = Some(
                                numbers
                                    .split([',', ' '])
                                    .filter_map(|x| x.trim().trim_end_matches('.').parse().ok())
                                    .collect(),
                            );
                        } else {
                            ending_stop[i] = true;
                        }
                    }
                }
                RawElement::Sound(s) => merge_sound(&mut sounds[i], s),
                RawElement::Direction(d) => {
                    if let Some(s) = &d.sound {
                        merge_sound(&mut sounds[i], s);
                    }
                    for k in &d.kinds {
                        match k {
                            RawDirectionKind::Segno => segno = Some(i),
                            RawDirectionKind::Coda => coda.push(i),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        if sounds[i].segno {
            segno = Some(i);
        }
        if sounds[i].coda {
            coda.push(i);
        }
    }

    let mut order = Vec::with_capacity(n);
    let mut i = 0;
    let mut section_start = 0;
    let mut pass = 1u32;
    let mut done: HashMap<usize, u32> = HashMap::new();
    let mut jumped = false;
    while i < n && order.len() < n * 20 {
        if forward[i] && section_start != i {
            section_start = i;
            pass = 1;
        }
        if let Some(numbers) = &ending_start[i]
            && !numbers.is_empty()
            && !numbers.contains(&pass)
        {
            // Skip this volta
            let mut j = i;
            while j < n && !ending_stop[j] {
                j += 1;
            }
            i = j + 1;
            continue;
        }

        order.push(i);

        if jumped && sounds[i].fine {
            break;
        }
        if let Some(times) = backward[i]
            && !jumped
        {
            let count = done.entry(i).or_insert(0);
            if *count < times - 1 {
                *count += 1;
                pass += 1;
                i = section_start;
                continue;
            }
            pass = 1;
            section_start = i + 1;
        }
        if !jumped && sounds[i].dacapo {
            jumped = true;
            i = 0;
            section_start = 0;
            pass = 1;
            continue;
        }
        if !jumped && sounds[i].dalsegno {
            jumped = true;
            i = segno.unwrap_or(0);
            continue;
        }
        if jumped
            && sounds[i].tocoda
            && let Some(&c) = coda.iter().find(|&&c| c > i)
        {
            i = c;
            continue;
        }
        i += 1;
    }
    order
}

fn merge_sound(into: &mut RawSound, s: &RawSound) {
    into.dacapo |= s.dacapo;
    into.dalsegno |= s.dalsegno;
    into.tocoda |= s.tocoda;
    into.fine |= s.fine;
    into.segno |= s.segno;
    into.coda |= s.coda;
}

fn note_value(kind: Option<&str>) -> Option<NoteValue> {
    Some(match kind? {
        "breve" | "long" | "whole" => NoteValue::Whole,
        "half" => NoteValue::Half,
        "quarter" => NoteValue::Quarter,
        "eighth" => NoteValue::Eighth,
        "16th" => NoteValue::Sixteenth,
        "32nd" => NoteValue::ThirtySecond,
        "64th" | "128th" | "256th" => NoteValue::SixtyFourth,
        _ => return None,
    })
}

fn accidental(name: &str) -> Option<Accidental> {
    Some(match name {
        "sharp" => Accidental::Sharp,
        "flat" => Accidental::Flat,
        "natural" => Accidental::Natural,
        "double-sharp" | "sharp-sharp" => Accidental::DoubleSharp,
        "flat-flat" => Accidental::DoubleFlat,
        _ => return None,
    })
}

/// MIDI pitch, diatonic step and alteration
fn pitch_of(letter: i32, alter: f32, octave: i32) -> (u8, i32, i8) {
    const SEMITONES: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    let alter = alter.round() as i32;
    let midi = (octave + 1) * 12 + SEMITONES[letter as usize] + alter;
    (midi.clamp(0, 127) as u8, octave * 7 + letter, alter as i8)
}

fn velocity_for(dynamic: &str) -> Option<u8> {
    Some(match dynamic {
        "pppppp" | "ppppp" | "pppp" => 20,
        "ppp" => 28,
        "pp" => 38,
        "p" => 50,
        "mp" => 64,
        "mf" => 78,
        "f" | "fp" => 92,
        "ff" => 106,
        "fff" => 116,
        "ffff" | "fffff" | "ffffff" => 124,
        _ => return None,
    })
}

/// Sudden accents that don't change the level
fn is_accent_dynamic(dynamic: &str) -> bool {
    matches!(
        dynamic,
        "sf" | "sfz" | "sffz" | "fz" | "rf" | "rfz" | "sfp" | "sfpp" | "sfzp"
    )
}

pub fn build(raw: RawScore, name: String) -> Result<MidiFile, String> {
    if raw.parts.is_empty() {
        return Err("The score has no parts".into());
    }

    // Global staves: the first two staves over all parts
    let mut staff_map: HashMap<(usize, usize), usize> = HashMap::new();
    let mut next_staff = 0;
    for (p, part) in raw.parts.iter().enumerate() {
        let mut staves = 1;
        for m in &part.measures {
            for e in &m.elements {
                match e {
                    RawElement::Attributes(a) => staves = staves.max(a.staves.unwrap_or(1)),
                    RawElement::Note(n) => staves = staves.max(n.staff),
                    _ => {}
                }
            }
        }
        let has_notes = part.measures.iter().any(|m| {
            m.elements
                .iter()
                .any(|e| matches!(e, RawElement::Note(n) if n.pitch.is_some()))
        });
        if !has_notes {
            continue;
        }
        for s in 1..=staves {
            if next_staff < 2 {
                staff_map.insert((p, s), next_staff);
                next_staff += 1;
            }
        }
    }
    if staff_map.is_empty() {
        return Err("The score has no notes".into());
    }

    // Tick resolution: fine enough for every divisions value and for triplets
    let mut ppq = 96u64;
    for part in &raw.parts {
        for m in &part.measures {
            for e in &m.elements {
                if let RawElement::Attributes(a) = e
                    && let Some(d) = a.divisions
                    && d > 0
                {
                    ppq = lcm(ppq, d);
                }
            }
        }
    }
    while ppq < 480 {
        ppq *= 2;
    }
    if ppq > MAX_PPQ {
        ppq = 960;
    }

    let order = play_order(&raw);

    let mut placed: Vec<Placed> = Vec::new();
    let mut directions: Vec<PlacedDirection> = Vec::new();
    let mut tempos: Vec<(u64, f64)> = Vec::new();
    let mut sound_dynamics: Vec<(u64, usize, f64)> = Vec::new();
    let mut infos: Vec<MeasureInfo> = Vec::new();
    let mut clef_timeline: [Vec<(u64, Clef)>; 2] = [vec![(0, Clef::Treble)], vec![(0, Clef::Bass)]];

    let mut divisions = vec![1u64; raw.parts.len()];
    let mut time = (4u8, 4u8);
    let mut key = 0i8;
    let mut measure_start = 0u64;
    let mut clefs = [Clef::Treble, Clef::Bass];

    for (u, &r) in order.iter().enumerate() {
        let mut max_cursor = 0u64;
        let mut info = MeasureInfo {
            start: measure_start,
            number: raw.parts[0].measures[r].number.clone(),
            ..Default::default()
        };
        let mut implicit = false;
        let mut clefs_at_start = clefs;

        for (p, part) in raw.parts.iter().enumerate() {
            let Some(m) = part.measures.get(r) else {
                continue;
            };
            implicit |= m.implicit;
            let mut cursor = 0u64;
            let mut last_start = 0u64;
            for element in &m.elements {
                let scale = |d: u64, div: u64| d * ppq / div.max(1);
                match element {
                    RawElement::Note(note) => {
                        let d = scale(note.duration, divisions[p]);
                        let start = if note.chord { last_start } else { cursor };
                        if !note.chord {
                            last_start = cursor;
                            if !note.grace {
                                cursor += d;
                            }
                        }
                        if let Some(&staff) = staff_map.get(&(p, note.staff)) {
                            placed.push(Placed {
                                measure: u,
                                staff,
                                voice: (p, note.voice),
                                start: measure_start + start,
                                duration: if note.grace { 0 } else { d },
                                note: (**note).clone(),
                            });
                        }
                    }
                    RawElement::Backup(d) => {
                        cursor = cursor.saturating_sub(scale(*d, divisions[p]));
                    }
                    RawElement::Forward(d) => cursor += scale(*d, divisions[p]),
                    RawElement::Attributes(a) => {
                        if let Some(d) = a.divisions {
                            divisions[p] = d.max(1);
                        }
                        if p == 0 {
                            if let Some(f) = a.fifths {
                                key = f.clamp(-7, 7);
                            }
                            if let Some(t) = a.time {
                                time = (t.0.max(1), t.1.max(1));
                            }
                        }
                        for &(s, clef) in &a.clefs {
                            if let Some(&staff) = staff_map.get(&(p, s)) {
                                clefs[staff] = clef;
                                if cursor == 0 {
                                    clefs_at_start[staff] = clef;
                                }
                                clef_timeline[staff].push((measure_start + cursor, clef));
                            }
                        }
                    }
                    RawElement::Direction(d) => {
                        let offset = d.offset * ppq as i64 / divisions[p].max(1) as i64;
                        let tick = (measure_start as i64 + cursor as i64 + offset).max(0) as u64;
                        let staff = staff_map.get(&(p, d.staff)).copied().unwrap_or(0);
                        for kind in &d.kinds {
                            directions.push(PlacedDirection {
                                measure: u,
                                staff,
                                tick,
                                above: d.above,
                                kind: kind.clone(),
                            });
                        }
                        if let Some(s) = &d.sound {
                            if let Some(t) = s.tempo {
                                tempos.push((tick, t));
                            }
                            if let Some(v) = s.dynamics {
                                sound_dynamics.push((tick, staff, v));
                            }
                        }
                    }
                    RawElement::Sound(s) => {
                        if let Some(t) = s.tempo {
                            tempos.push((measure_start + cursor, t));
                        }
                    }
                    RawElement::Barline(b) if p == 0 => {
                        if let Some((direction, _)) = &b.repeat {
                            if direction == "forward" {
                                info.repeat_start = true;
                            } else {
                                info.repeat_end = true;
                            }
                        }
                        if let Some((numbers, kind)) = &b.ending
                            && kind == "start"
                        {
                            info.ending = Some(format!("{}.", numbers.trim_end_matches('.')));
                        }
                    }
                    RawElement::Barline(_) => {}
                }
                max_cursor = max_cursor.max(cursor);
            }
        }

        // The time signature decides the length; only pickups use their content
        let nominal = time.0 as u64 * 4 * ppq / time.1 as u64;
        // Shorter content: a pickup or its complement before a repeat (exporters write
        // out all rests, so short really means short). Longer content: a free-length
        // measure, rounded to an eighth of a quarter to absorb rounding in the file.
        let _ = r;
        let step = (ppq / 8).max(1);
        let len = if implicit || (max_cursor > 0 && max_cursor < nominal) {
            max_cursor.max(1)
        } else if max_cursor > nominal + step {
            (max_cursor + step / 2 - 1) / step * step
        } else {
            nominal
        };
        info.len = len;
        info.time = time;
        info.key = key;
        info.clefs = clefs_at_start;
        infos.push(info);
        measure_start += len;
    }

    let total_ticks = measure_start;
    for timeline in clef_timeline.iter_mut() {
        timeline.sort_by_key(|(t, _)| *t);
    }
    let clef_at = |staff: usize, tick: u64| -> Clef {
        let timeline = &clef_timeline[staff];
        let i = timeline.partition_point(|(t, _)| *t <= tick);
        timeline[i.saturating_sub(1)].1
    };

    // ---- Playback ----------------------------------------------------------------------

    // Dynamic level per staff over time
    let mut levels: [Vec<(u64, u8)>; 2] = [vec![(0, 78)], vec![(0, 78)]];
    let mut accents: Vec<(u64, usize)> = Vec::new();
    for d in &directions {
        if let RawDirectionKind::Dynamic(name) = &d.kind {
            if let Some(v) = velocity_for(name) {
                // Dynamics usually apply to both hands
                for level in levels.iter_mut() {
                    level.push((d.tick, v));
                }
            } else if is_accent_dynamic(name) {
                accents.push((d.tick, d.staff));
            }
        }
    }
    for &(tick, _, pct) in &sound_dynamics {
        let v = (pct * 0.9).round().clamp(1.0, 127.0) as u8;
        for level in levels.iter_mut() {
            level.push((tick, v));
        }
    }
    for level in levels.iter_mut() {
        level.sort_by_key(|(t, _)| *t);
    }
    let level_at = |staff: usize, tick: u64| -> u8 {
        let l = &levels[staff];
        let i = l.partition_point(|(t, _)| *t <= tick);
        l[i.saturating_sub(1)].1
    };

    // Merge tied notes, keep the chain each placed note belongs to
    let mut sounding: Vec<Sounding> = Vec::new();
    let mut chain_of: Vec<Option<usize>> = vec![None; placed.len()];
    let mut open_ties: HashMap<(usize, u8), usize> = HashMap::new();

    // Slashed grace notes are played just before the beat, others on the beat
    // (taking time from the main note)
    let grace_len = (ppq / 12).max(1);
    let appoggiatura_len = (ppq / 8).max(1);
    let mut pending_graces: HashMap<(usize, u32), Vec<usize>> = HashMap::new();

    let mut sorted: Vec<usize> = (0..placed.len()).collect();
    sorted.sort_by_key(|&i| (placed[i].start, placed[i].note.grace as u8 ^ 1));

    for &i in &sorted {
        let p = &placed[i];
        let Some((letter, alter, octave)) = p.note.pitch else {
            continue;
        };
        if p.note.rest {
            continue;
        }
        let (pitch, _, _) = pitch_of(letter, alter, octave);

        if p.note.grace {
            pending_graces
                .entry((p.voice.0, p.voice.1))
                .or_default()
                .push(i);
            continue;
        }

        if p.note.tie_stop
            && let Some(&c) = open_ties.get(&(p.staff, pitch))
            && sounding[c].end >= p.start.saturating_sub(ppq / 8)
        {
            sounding[c].end = p.start + p.duration;
            chain_of[i] = Some(c);
            if !p.note.tie_start {
                open_ties.remove(&(p.staff, pitch));
            }
            continue;
        }

        let mut velocity = level_at(p.staff, p.start) as i32;
        if p.note.accent {
            velocity += 12;
        }
        if p.note.marcato {
            velocity += 18;
        }
        if accents.iter().any(|&(t, s)| t == p.start && s == p.staff) {
            velocity += 20;
        }

        // Grace notes waiting for this note
        let graces = pending_graces
            .remove(&(p.voice.0, p.voice.1))
            .unwrap_or_default();
        let on_beat = !graces.is_empty() && graces.iter().all(|&g| !placed[g].note.grace_slash);
        let delay = if on_beat {
            (graces.len() as u64 * appoggiatura_len).min(p.duration / 2)
        } else {
            0
        };

        let mut length = p.duration.saturating_sub(delay);
        if !p.note.tie_start {
            if p.note.staccatissimo {
                length = length * 3 / 10;
            } else if p.note.staccato {
                length /= 2;
            }
        }
        let c = sounding.len();
        sounding.push(Sounding {
            staff: p.staff,
            pitch,
            start: p.start + delay,
            end: p.start + delay + length.max(ppq / 16),
            velocity: velocity.clamp(1, 127) as u8,
        });
        chain_of[i] = Some(c);
        if p.note.tie_start {
            open_ties.insert((p.staff, pitch), c);
        }

        {
            let count = graces.len() as u64;
            for (k, &g) in graces.iter().enumerate() {
                let gp = &placed[g];
                let Some((l, a, o)) = gp.note.pitch else {
                    continue;
                };
                let (gpitch, _, _) = pitch_of(l, a, o);
                let (start, grace_len) = if on_beat {
                    (
                        p.start + k as u64 * (delay / count.max(1)),
                        delay / count.max(1),
                    )
                } else {
                    (
                        p.start.saturating_sub((count - k as u64) * grace_len),
                        grace_len,
                    )
                };
                let gc = sounding.len();
                sounding.push(Sounding {
                    staff: gp.staff,
                    pitch: gpitch,
                    start,
                    end: start + grace_len,
                    velocity: (velocity - 10).clamp(1, 127) as u8,
                });
                chain_of[g] = Some(gc);
            }
        }
    }

    // Rolled chords: spread the notes of arpeggiated chords
    let roll = (ppq / 24).max(1);
    let mut rolled: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, p) in placed.iter().enumerate() {
        if p.note.arpeggiate
            && !p.note.grace
            && let Some(c) = chain_of[i]
        {
            rolled.entry((p.staff, p.start)).or_default().push(c);
        }
    }
    for chain in rolled.values_mut() {
        chain.sort_by_key(|&c| sounding[c].pitch);
        chain.dedup();
        for (k, &c) in chain.iter().enumerate() {
            let shift = k as u64 * roll;
            sounding[c].start += shift;
            sounding[c].end = sounding[c].end.max(sounding[c].start + ppq / 16);
        }
    }

    // Pedal
    let mut pedal_spans: Vec<(u64, u64)> = Vec::new();
    let mut pedal_down: Option<u64> = None;
    let mut pedal_marks: Vec<(u64, String)> = directions
        .iter()
        .filter_map(|d| match &d.kind {
            RawDirectionKind::Pedal(kind) => Some((d.tick, kind.clone())),
            _ => None,
        })
        .collect();
    pedal_marks.sort_by(|a, b| a.0.cmp(&b.0));
    for (tick, kind) in pedal_marks {
        match kind.as_str() {
            "start" => {
                if pedal_down.is_none() {
                    pedal_down = Some(tick);
                }
            }
            "change" => {
                if let Some(start) = pedal_down
                    && tick > start
                {
                    pedal_spans.push((start, tick));
                }
                pedal_down = Some(tick);
            }
            "stop" => {
                if let Some(start) = pedal_down.take()
                    && tick > start
                {
                    pedal_spans.push((start, tick));
                }
            }
            _ => {}
        }
    }
    if let Some(start) = pedal_down {
        pedal_spans.push((start, total_ticks));
    }

    tempos.sort_by(|a, b| a.0.cmp(&b.0));
    if tempos.first().is_none_or(|(t, _)| *t > 0) {
        tempos.insert(0, (0, tempos.first().map_or(DEFAULT_TEMPO, |t| t.1)));
    }

    let smf = playback_smf(
        ppq,
        &infos,
        &tempos,
        &sounding_events(&sounding),
        &pedal_spans,
    );
    let mut file = MidiFile::from_smf_unsplit(name, &smf)?;

    // Hands come from the staves
    let hands: Vec<Option<Hand>> = file
        .tracks
        .iter()
        .map(|t| match t.track_id {
            1 => Some(Hand::Right),
            2 => Some(Hand::Left),
            _ => None,
        })
        .collect();
    file.tracks = crate::hands::set_hands(file.tracks.to_vec(), &hands).into();

    // ---- Notation ----------------------------------------------------------------------

    let tempo = file.tempo_track.clone();
    let time_of = |tick: u64| tempo.pulses_to_duration(tick);
    let note_times = |i: usize| -> (std::time::Duration, std::time::Duration) {
        match chain_of[i] {
            Some(c) => (time_of(sounding[c].start), time_of(sounding[c].end)),
            None => (
                time_of(placed[i].start),
                time_of(placed[i].start + placed[i].duration),
            ),
        }
    };

    // Octave shifts per staff: (start, stop, +1 for 8va / -1 for 8vb)
    let mut ottavas: [Vec<(u64, u64, i8)>; 2] = [Vec::new(), Vec::new()];
    let mut open_shift: HashMap<(usize, u32), (u64, i8)> = HashMap::new();
    // Some exports lose the start marks; then the file's 8va can't be trusted
    let mut unmatched_stops = 0;
    let mut shift_marks: Vec<&PlacedDirection> = directions
        .iter()
        .filter(|d| matches!(d.kind, RawDirectionKind::OctaveShift(..)))
        .collect();
    shift_marks.sort_by_key(|d| d.tick);
    for d in shift_marks {
        if let RawDirectionKind::OctaveShift(kind, size, number) = &d.kind {
            if *size != 8 && *size != 15 {
                continue;
            }
            match kind.as_str() {
                "down" => {
                    open_shift.insert((d.staff, *number), (d.tick, 1));
                }
                "up" => {
                    open_shift.insert((d.staff, *number), (d.tick, -1));
                }
                _ => {
                    if let Some((start, dir)) = open_shift.remove(&(d.staff, *number)) {
                        ottavas[d.staff].push((start, d.tick, dir));
                    } else {
                        unmatched_stops += 1;
                    }
                }
            }
        }
    }
    let ottava_at = |staff: usize, tick: u64| -> i8 {
        ottavas[staff]
            .iter()
            .find(|(a, b, _)| *a <= tick && tick <= *b)
            .map_or(0, |(_, _, d)| *d)
    };

    // Group placed notes by (measure, staff, voice)
    let mut groups: BTreeMap<(usize, usize, (usize, u32)), Vec<usize>> = BTreeMap::new();
    for (i, p) in placed.iter().enumerate() {
        groups
            .entry((p.measure, p.staff, p.voice))
            .or_default()
            .push(i);
    }

    let mut measures: Vec<Measure> = infos
        .iter()
        .enumerate()
        .map(|(u, info)| {
            let previous = u.checked_sub(1).map(|i| &infos[i]);
            Measure {
                index: u,
                start_tick: info.start,
                end_tick: info.start + info.len,
                start: time_of(info.start),
                end: time_of(info.start + info.len),
                time_signature: info.time,
                key: info.key,
                previous_key: previous.filter(|p| p.key != info.key).map(|p| p.key),
                time_signature_changed: previous.is_some_and(|p| p.time != info.time),
                staves: [0, 1].map(|s| StaffMeasure {
                    clef: info.clefs[s],
                    clef_changes: clef_timeline[s]
                        .iter()
                        .filter(|(t, _)| *t > info.start && *t < info.start + info.len)
                        .copied()
                        .collect(),
                    ..Default::default()
                }),
                pedal: pedal_spans
                    .iter()
                    .filter(|(a, b)| *a < info.start + info.len && *b > info.start)
                    .map(|&(a, b)| PedalSpan {
                        start: a.max(info.start),
                        end: b.min(info.start + info.len),
                        continued: a < info.start,
                        continues: b > info.start + info.len,
                    })
                    .collect(),
                number: info.number.clone(),
                repeat_start: info.repeat_start,
                repeat_end: info.repeat_end,
                ending: info.ending.clone(),
                directions: Vec::new(),
            }
        })
        .collect();

    let mut event_of: Vec<Option<EventRef>> = vec![None; placed.len()];

    for ((u, staff, _), members) in &groups {
        let (u, staff) = (*u, *staff);
        let mut voice = Voice::default();
        let mut graces: Vec<ScoreNote> = Vec::new();
        let mut open_tuplet: Option<(usize, u8, u8, Option<bool>)> = None;
        let mut open_beam: Option<usize> = None;
        let voice_index = measures[u].staves[staff].voices.len();

        for &i in members {
            let p = &placed[i];
            let n = &p.note;
            if n.rest && !n.printed {
                continue;
            }

            let score_note = n.pitch.map(|(letter, alter, octave)| {
                let (pitch, step, alter_i) = pitch_of(letter, alter, octave);
                let (start, end) = note_times(i);
                ScoreNote {
                    pitch,
                    step,
                    alter: alter_i,
                    accidental: n.accidental.as_deref().and_then(accidental),
                    start,
                    end,
                    hand: Some(if staff == 0 { Hand::Right } else { Hand::Left }),
                    tie_from_prev: n.tie_stop,
                    tie_to_next: n.tie_start,
                    trill: n.ornament == Some(Ornament::Trill),
                    fingering: n.fingering.clone(),
                    fingering_auto: false,
                }
            });

            if n.grace {
                if let Some(note) = score_note {
                    graces.push(note);
                }
                continue;
            }

            let joins_chord = n.chord
                && voice
                    .events
                    .last()
                    .is_some_and(|e| e.tick == p.start && !e.is_rest());
            if joins_chord {
                let e = voice.events.last_mut().unwrap();
                if let Some(note) = score_note {
                    e.notes.push(note);
                    e.notes.sort_by_key(|n| n.pitch);
                }
                e.arpeggio |= n.arpeggiate;
                e.staccato |= n.staccato;
                event_of[i] = Some(EventRef {
                    measure: u,
                    staff,
                    voice: voice_index,
                    event: voice.events.len() - 1,
                });
                continue;
            }

            let index = voice.events.len();
            let value = note_value(n.kind.as_deref())
                .unwrap_or_else(|| NoteValue::closest(p.duration.max(1), ppq as u16).0);
            let mut articulations = Vec::new();
            if n.accent {
                articulations.push(Articulation::Accent);
            }
            if n.marcato {
                articulations.push(Articulation::Marcato);
            }
            if n.tenuto {
                articulations.push(Articulation::Tenuto);
            }
            if n.staccatissimo {
                articulations.push(Articulation::Staccatissimo);
            }
            if n.fermata {
                articulations.push(Articulation::Fermata);
            }

            voice.events.push(Event {
                tick: p.start,
                ticks: p.duration,
                value: if n.measure_rest {
                    NoteValue::Whole
                } else {
                    value
                },
                dots: if n.measure_rest { 0 } else { n.dots },
                notes: score_note.into_iter().collect(),
                whole_measure_rest: n.measure_rest || (n.rest && n.kind.is_none()),
                ottava: ottava_at(staff, p.start),
                staccato: n.staccato,
                grace: std::mem::take(&mut graces),
                clef: clef_at(staff, p.start),
                stem: n.stem,
                articulations,
                ornament: n.ornament.filter(|o| *o != Ornament::Trill),
                arpeggio: n.arpeggiate,
                ..Default::default()
            });
            event_of[i] = Some(EventRef {
                measure: u,
                staff,
                voice: voice_index,
                event: index,
            });

            // Tuplets
            if let Some(bracket) = n.tuplet_start {
                let (actual, normal) = n.time_modification.unwrap_or((3, 2));
                open_tuplet = Some((index, actual as u8, normal as u8, bracket));
            }
            if n.tuplet_stop
                && let Some((first, actual, normal, bracket)) = open_tuplet.take()
            {
                let t = voice.tuplets.len();
                voice.tuplets.push(Tuplet {
                    actual,
                    normal,
                    bracket,
                    first,
                    last: index,
                });
                for e in &mut voice.events[first..=index] {
                    e.tuplet = Some(t);
                }
            }

            // Beams
            match n.beam.as_deref() {
                Some("begin") => open_beam = Some(index),
                Some("end") => {
                    if let Some(first) = open_beam.take()
                        && index > first
                    {
                        voice.beams.push(Beam { first, last: index });
                    }
                }
                _ => {}
            }
        }

        // Tuplets written without start/stop marks: group by the time modification
        tuplets_from_time_modification(&mut voice, members, &placed);

        if !voice.events.is_empty() {
            measures[u].staves[staff].voices.push(voice);
        }
    }

    // Staves without any voice get a whole measure rest
    for m in measures.iter_mut() {
        for (s, staff) in m.staves.iter_mut().enumerate() {
            if staff.voices.is_empty() {
                staff.voices.push(Voice {
                    events: vec![Event {
                        tick: m.start_tick,
                        ticks: m.end_tick - m.start_tick,
                        value: NoteValue::Whole,
                        whole_measure_rest: true,
                        clef: clef_at(s, m.start_tick),
                        ..Default::default()
                    }],
                    ..Default::default()
                });
            }
        }
    }

    // Directions: dynamics and words
    for d in &directions {
        let kind = match &d.kind {
            RawDirectionKind::Dynamic(name) => DirectionKind::Dynamic(name.clone()),
            RawDirectionKind::Words { text, italic, bold } => DirectionKind::Words {
                text: text.clone(),
                italic: *italic,
                bold: *bold,
            },
            _ => continue,
        };
        let above = d
            .above
            .unwrap_or(matches!(kind, DirectionKind::Words { .. }));
        if let Some(m) = measures.get_mut(d.measure) {
            m.directions.push(Direction {
                tick: d.tick.clamp(m.start_tick, m.end_tick.saturating_sub(1)),
                staff: d.staff,
                above,
                kind,
            });
        }
    }

    // Slurs: pair start and stop marks per part and number
    let mut slurs = Vec::new();
    let mut open_slurs: HashMap<(usize, u32), (EventRef, Option<bool>)> = HashMap::new();
    for &i in &sorted {
        let Some(at) = event_of[i] else { continue };
        for (kind, number, above) in &placed[i].note.slurs {
            let key = (placed[i].voice.0, *number);
            match kind.as_str() {
                "start" => {
                    open_slurs.insert(key, (at, *above));
                }
                "stop" => {
                    if let Some((start, above)) = open_slurs.remove(&key)
                        && start != at
                    {
                        slurs.push(Slur {
                            start,
                            end: at,
                            above,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // Wedges
    let mut wedges = Vec::new();
    let mut open_wedges: HashMap<u32, (usize, u64, usize, bool, bool)> = HashMap::new();
    for d in &directions {
        if let RawDirectionKind::Wedge(kind, number) = &d.kind {
            match kind.as_str() {
                "crescendo" | "diminuendo" => {
                    open_wedges.insert(
                        *number,
                        (
                            d.measure,
                            d.tick,
                            d.staff,
                            kind == "crescendo",
                            d.above.unwrap_or(false),
                        ),
                    );
                }
                _ => {
                    if let Some((m, tick, staff, crescendo, above)) = open_wedges.remove(number) {
                        wedges.push(Wedge {
                            staff,
                            start: (m, tick),
                            end: (d.measure, d.tick),
                            crescendo,
                            above,
                        });
                    }
                }
            }
        }
    }

    if unmatched_stops > 0 {
        for measure in measures.iter_mut() {
            for staff in measure.staves.iter_mut() {
                for voice in staff.voices.iter_mut() {
                    for event in voice.events.iter_mut() {
                        event.ottava = 0;
                    }
                }
            }
        }
        crate::score::auto_ottava(&mut measures);
    }

    file.score = Some(Arc::new(Score {
        ppq: ppq as u16,
        measures,
        grid_alignment: 1.0,
        slurs,
        wedges,
    }));
    Ok(file)
}

fn tuplets_from_time_modification(voice: &mut Voice, members: &[usize], placed: &[Placed]) {
    if !voice.tuplets.is_empty() {
        return;
    }
    // Time modification of each event (from its first note)
    let mods: Vec<Option<(u32, u32)>> = voice
        .events
        .iter()
        .map(|e| {
            members
                .iter()
                .find(|&&i| placed[i].start == e.tick && !placed[i].note.grace)
                .and_then(|&i| placed[i].note.time_modification)
        })
        .collect();

    let mut i = 0;
    while i < voice.events.len() {
        let Some((actual, normal)) = mods[i] else {
            i += 1;
            continue;
        };
        let mut j = i;
        while j + 1 < voice.events.len()
            && mods[j + 1] == Some((actual, normal))
            && j + 1 - i < actual as usize
        {
            j += 1;
        }
        let t = voice.tuplets.len();
        voice.tuplets.push(Tuplet {
            actual: actual as u8,
            normal: normal as u8,
            bracket: None,
            first: i,
            last: j,
        });
        for e in &mut voice.events[i..=j] {
            e.tuplet = Some(t);
        }
        i = j + 1;
    }
}

/// A note as it is played
struct Sounding {
    staff: usize,
    pitch: u8,
    start: u64,
    end: u64,
    velocity: u8,
}

/// (tick, staff, order, message) for all notes, note offs before note ons at the same tick
fn sounding_events(sounding: &[Sounding]) -> Vec<(u64, usize, u8, MidiMessage)> {
    let mut events = Vec::with_capacity(sounding.len() * 2);
    for s in sounding {
        let (staff, pitch, start, end, velocity) = (s.staff, s.pitch, s.start, s.end, s.velocity);
        events.push((
            start,
            staff,
            1,
            MidiMessage::NoteOn {
                key: u7::new(pitch),
                vel: u7::new(velocity),
            },
        ));
        events.push((
            end.max(start + 1),
            staff,
            0,
            MidiMessage::NoteOff {
                key: u7::new(pitch),
                vel: u7::new(0),
            },
        ));
    }
    events.sort_by_key(|(t, s, order, _)| (*t, *s, *order));
    events
}

fn playback_smf(
    ppq: u64,
    infos: &[MeasureInfo],
    tempos: &[(u64, f64)],
    notes: &[(u64, usize, u8, MidiMessage)],
    pedal: &[(u64, u64)],
) -> Smf<'static> {
    // Meta track: tempo, time and key signatures
    let mut meta: Vec<(u64, TrackEventKind<'static>)> = Vec::new();
    for &(tick, bpm) in tempos {
        let us = (60_000_000.0 / bpm.clamp(10.0, 1000.0)).round() as u32;
        meta.push((tick, TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us)))));
    }
    let mut last: Option<((u8, u8), i8)> = None;
    for info in infos {
        if last.is_none_or(|(t, _)| t != info.time) {
            let pow = (info.time.1 as f32).log2().round() as u8;
            meta.push((
                info.start,
                TrackEventKind::Meta(MetaMessage::TimeSignature(info.time.0, pow, 24, 8)),
            ));
        }
        if last.is_none_or(|(_, k)| k != info.key) {
            meta.push((
                info.start,
                TrackEventKind::Meta(MetaMessage::KeySignature(info.key, false)),
            ));
        }
        last = Some((info.time, info.key));
    }
    meta.sort_by_key(|(t, _)| *t);

    let mut tracks = vec![to_track(meta)];
    for staff in 0..2 {
        let mut events: Vec<(u64, TrackEventKind<'static>)> = notes
            .iter()
            .filter(|(_, s, _, _)| *s == staff)
            .map(|(t, _, _, m)| {
                (
                    *t,
                    TrackEventKind::Midi {
                        channel: u4::new(0),
                        message: *m,
                    },
                )
            })
            .collect();
        if staff == 1 || notes.iter().all(|(_, s, _, _)| *s == 0) {
            for &(a, b) in pedal {
                for (tick, value) in [(a, 127), (b, 0)] {
                    events.push((
                        tick,
                        TrackEventKind::Midi {
                            channel: u4::new(0),
                            message: MidiMessage::Controller {
                                controller: u7::new(64),
                                value: u7::new(value),
                            },
                        },
                    ));
                }
            }
            events.sort_by_key(|(t, _)| *t);
        }
        tracks.push(to_track(events));
    }

    Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(ppq as u16))),
        tracks,
    }
}

fn to_track(events: Vec<(u64, TrackEventKind<'static>)>) -> Vec<TrackEvent<'static>> {
    let mut out = Vec::with_capacity(events.len() + 1);
    let mut last = 0u64;
    for (tick, kind) in events {
        out.push(TrackEvent {
            delta: u28::new((tick - last) as u32),
            kind,
        });
        last = tick;
    }
    out.push(TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    out
}
