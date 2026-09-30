use midi_file::{
    MidiFile,
    midly::{
        Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
        num::{u4, u7, u15, u28},
    },
    score::{Accidental, Event, NoteValue, Score, Voice},
};

const PPQ: u32 = 480;
const Q: u32 = PPQ;
const E: u32 = PPQ / 2;
const S: u32 = PPQ / 4;

fn ev(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
    TrackEvent {
        delta: u28::new(delta),
        kind,
    }
}

fn midi(message: MidiMessage) -> TrackEventKind<'static> {
    TrackEventKind::Midi {
        channel: u4::new(0),
        message,
    }
}

/// Track from notes (start, length, key) and pedal changes (tick, down)
fn track_with_pedal(notes: &[(u32, u32, u8)], pedal: &[(u32, bool)]) -> Vec<TrackEvent<'static>> {
    // (tick, order, message): note offs first, then pedal, then note ons
    let mut points: Vec<(u32, u8, MidiMessage)> = Vec::new();
    for &(start, len, key) in notes {
        points.push((
            start,
            2,
            MidiMessage::NoteOn {
                key: u7::new(key),
                vel: u7::new(80),
            },
        ));
        points.push((
            start + len,
            0,
            MidiMessage::NoteOff {
                key: u7::new(key),
                vel: u7::new(0),
            },
        ));
    }
    for &(tick, down) in pedal {
        points.push((
            tick,
            1,
            MidiMessage::Controller {
                controller: u7::new(64),
                value: u7::new(if down { 127 } else { 0 }),
            },
        ));
    }
    points.sort_by_key(|(t, order, _)| (*t, *order));

    let mut events = Vec::new();
    let mut last = 0;
    for (t, _, message) in points {
        events.push(ev(t - last, midi(message)));
        last = t;
    }
    events.push(ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
    events
}

fn track(notes: &[(u32, u32, u8)]) -> Vec<TrackEvent<'static>> {
    track_with_pedal(notes, &[])
}

fn meta(time_sig: Option<(u8, u8)>, key: Option<i8>) -> Vec<TrackEvent<'static>> {
    let mut events = Vec::new();
    if let Some((num, denom_pow)) = time_sig {
        events.push(ev(
            0,
            TrackEventKind::Meta(MetaMessage::TimeSignature(num, denom_pow, 24, 8)),
        ));
    }
    if let Some(sharps) = key {
        events.push(ev(
            0,
            TrackEventKind::Meta(MetaMessage::KeySignature(sharps, false)),
        ));
    }
    events.push(ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
    events
}

/// Score from a right hand (treble) and optional left hand (bass) track
fn score(meta: Vec<TrackEvent<'static>>, tracks: Vec<Vec<TrackEvent<'static>>>) -> Score {
    let mut all = vec![meta];
    all.extend(tracks);
    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ as u16))),
        tracks: all,
    };
    Score::new(&MidiFile::from_smf("test", &smf).unwrap())
}

fn four_four() -> Vec<TrackEvent<'static>> {
    meta(Some((4, 2)), None)
}

fn voice(score: &Score, measure: usize, staff: usize, v: usize) -> &Voice {
    &score.measures[measure].staves[staff].voices[v]
}

/// (tick relative to the measure, value, dots, pitches) of a voice
fn summary(
    score: &Score,
    measure: usize,
    staff: usize,
    v: usize,
) -> Vec<(u32, NoteValue, u8, Vec<u8>)> {
    let start = score.measures[measure].start_tick;
    voice(score, measure, staff, v)
        .events
        .iter()
        .map(|e: &Event| {
            (
                (e.tick - start) as u32,
                e.value,
                e.dots,
                e.notes.iter().map(|n| n.pitch).collect(),
            )
        })
        .collect()
}

#[test]
fn quarters_chords_and_rests() {
    // Right hand: C5 quarter, (rest), E5+G5 half. Left hand: C3 whole
    let right = track(&[(0, Q, 72), (2 * Q, 2 * Q, 76), (2 * Q, 2 * Q, 79)]);
    let left = track(&[(0, 4 * Q, 48)]);
    let score = score(four_four(), vec![right, left]);

    assert_eq!(score.measures.len(), 1);
    assert!(score.is_readable());
    assert_eq!(
        summary(&score, 0, 0, 0),
        vec![
            (0, NoteValue::Quarter, 0, vec![72]),
            (Q, NoteValue::Quarter, 0, vec![]),
            (2 * Q, NoteValue::Half, 0, vec![76, 79]),
        ]
    );
    assert_eq!(
        summary(&score, 0, 1, 0),
        vec![(0, NoteValue::Whole, 0, vec![48])]
    );
}

#[test]
fn time_signature_and_empty_staff() {
    // 3/4, right hand only
    let right = track(&[(0, 3 * Q, 72), (3 * Q, 3 * Q, 74)]);
    let score = score(meta(Some((3, 2)), None), vec![right]);

    assert_eq!(score.measures.len(), 2);
    assert_eq!(score.measures[1].start_tick, 3 * Q as u64);
    assert_eq!(
        summary(&score, 0, 0, 0),
        vec![(0, NoteValue::Half, 1, vec![72])]
    );
    let bass = &voice(&score, 0, 1, 0).events;
    assert_eq!(bass.len(), 1);
    assert!(bass[0].whole_measure_rest);
}

#[test]
fn accidentals_follow_key_and_measure() {
    // G major: F#5 nothing, F5 natural, F#5 sharp again, C#5 sharp, C#5 nothing
    let right = track(&[
        (0, E, 78),
        (E, E, 77),
        (2 * E, E, 78),
        (3 * E, E, 73),
        (4 * E, 4 * E, 73),
    ]);
    let score = score(meta(Some((4, 2)), Some(1)), vec![right]);

    let accidentals: Vec<Option<Accidental>> = voice(&score, 0, 0, 0)
        .events
        .iter()
        .filter(|e| !e.is_rest())
        .map(|e| e.notes[0].accidental)
        .collect();
    assert_eq!(
        accidentals,
        vec![
            None,
            Some(Accidental::Natural),
            Some(Accidental::Sharp),
            Some(Accidental::Sharp),
            None
        ]
    );
}

#[test]
fn flat_keys_spell_with_flats() {
    // F major: Bb4 in the key, Eb5 needs a flat
    let right = track(&[(0, Q, 70), (Q, 3 * Q, 75)]);
    let score = score(meta(Some((4, 2)), Some(-1)), vec![right]);
    let notes: Vec<(i32, Option<Accidental>)> = voice(&score, 0, 0, 0)
        .events
        .iter()
        .flat_map(|e| e.notes.iter().map(|n| (n.step, n.accidental)))
        .collect();
    // B4 = 4*7+6, E5 = 5*7+2
    assert_eq!(notes[0], (34, None));
    assert_eq!(notes[1], (37, Some(Accidental::Flat)));
    assert!(
        notes[2..]
            .iter()
            .all(|(step, acc)| *step == 37 && acc.is_none())
    );
}

#[test]
fn note_crossing_barline_is_tied() {
    let right = track(&[(3 * Q, 2 * Q, 72), (5 * Q, 3 * Q, 74)]);
    let score = score(four_four(), vec![right]);

    let first = voice(&score, 0, 0, 0).events.last().unwrap();
    assert_eq!(first.value, NoteValue::Quarter);
    assert!(first.notes[0].tie_to_next);

    let second = &voice(&score, 1, 0, 0).events[0];
    assert_eq!(second.value, NoteValue::Quarter);
    assert!(second.notes[0].tie_from_prev);
    assert!(second.notes[0].accidental.is_none());
}

#[test]
fn syncopation_across_the_middle_of_the_bar_is_split() {
    // Quarter, half starting on beat 2, quarter: the half crosses the middle of 4/4
    let right = track(&[(0, Q, 72), (Q, 2 * Q, 74), (3 * Q, Q, 76)]);
    let score = score(four_four(), vec![right]);
    assert_eq!(
        summary(&score, 0, 0, 0),
        vec![
            (0, NoteValue::Quarter, 0, vec![72]),
            (Q, NoteValue::Quarter, 0, vec![74]),
            (2 * Q, NoteValue::Quarter, 0, vec![74]),
            (3 * Q, NoteValue::Quarter, 0, vec![76]),
        ]
    );
    let events = &voice(&score, 0, 0, 0).events;
    assert!(events[1].notes[0].tie_to_next);
    assert!(events[2].notes[0].tie_from_prev);
}

#[test]
fn triplets_are_detected_and_beamed() {
    let t = Q / 3;
    // Beat 1: triplet eighths, beat 2: two eighths, beats 3-4: half
    let right = track(&[
        (0, t, 72),
        (t, t, 74),
        (2 * t, t, 76),
        (Q, E, 77),
        (Q + E, E, 79),
        (2 * Q, 2 * Q, 81),
    ]);
    let score = score(four_four(), vec![right]);
    let v = voice(&score, 0, 0, 0);

    assert_eq!(v.tuplets.len(), 1);
    let tuplet = &v.tuplets[0];
    assert_eq!((tuplet.first, tuplet.last, tuplet.actual), (0, 2, 3));
    assert!(v.events[..3].iter().all(|e| e.value == NoteValue::Eighth));
    assert!(v.events[3..].iter().all(|e| e.tuplet.is_none()));

    let beams: Vec<(usize, usize)> = v.beams.iter().map(|b| (b.first, b.last)).collect();
    assert_eq!(beams, vec![(0, 2), (3, 4)]);
}

#[test]
fn eighths_beam_by_half_bar_and_sixteenths_by_beat() {
    let mut notes: Vec<(u32, u32, u8)> = (0..4).map(|i| (i * E, E, 72 + i as u8)).collect();
    notes.extend((0..8).map(|i| (2 * Q + i * S, S, 72 + i as u8)));
    let score = score(four_four(), vec![track(&notes)]);
    let beams: Vec<(usize, usize)> = voice(&score, 0, 0, 0)
        .beams
        .iter()
        .map(|b| (b.first, b.last))
        .collect();
    assert_eq!(beams, vec![(0, 3), (4, 7), (8, 11)]);
}

#[test]
fn sustained_note_gets_its_own_voice() {
    // Left hand: C3 half note held under moving quarters E3 G3
    let left = track(&[(0, 2 * Q, 48), (0, Q, 52), (Q, Q, 55), (2 * Q, 2 * Q, 48)]);
    let right = track(&[(0, 4 * Q, 72)]);
    let score = score(four_four(), vec![right, left]);

    let staff = &score.measures[0].staves[1];
    assert_eq!(staff.voices.len(), 2);
    assert_eq!(
        summary(&score, 0, 1, 0),
        vec![
            (0, NoteValue::Quarter, 0, vec![52]),
            (Q, NoteValue::Quarter, 0, vec![55]),
            (2 * Q, NoteValue::Half, 0, vec![48]),
        ]
    );
    let lower = summary(&score, 0, 1, 1);
    assert_eq!(lower[0], (0, NoteValue::Half, 0, vec![48]));
}

#[test]
fn detached_notes_are_staccato_not_rests() {
    // Eighths sounding a 16th each
    let notes: Vec<(u32, u32, u8)> = (0..8).map(|i| (i * E, S, 72)).collect();
    let score = score(four_four(), vec![track(&notes)]);
    let v = voice(&score, 0, 0, 0);
    assert_eq!(v.events.len(), 8);
    assert!(
        v.events
            .iter()
            .all(|e| !e.is_rest() && e.value == NoteValue::Eighth)
    );
    assert!(v.events.iter().all(|e| e.staccato));
}

#[test]
fn rolled_chord_is_one_chord() {
    let right = track(&[(0, 4 * Q, 72), (10, 4 * Q - 10, 76), (20, 4 * Q - 20, 79)]);
    let score = score(four_four(), vec![right]);
    assert_eq!(
        summary(&score, 0, 0, 0),
        vec![(0, NoteValue::Whole, 0, vec![72, 76, 79])]
    );
}

#[test]
fn pedal_spans_per_measure() {
    let right = track_with_pedal(
        &[(0, 8 * Q, 72)],
        &[(10, true), (2 * Q, false), (2 * Q, true), (6 * Q, false)],
    );
    let score = score(four_four(), vec![right]);
    let spans: Vec<(u64, u64, bool, bool)> = score
        .measures
        .iter()
        .flat_map(|m| {
            m.pedal
                .iter()
                .map(|p| (p.start, p.end, p.continued, p.continues))
        })
        .collect();
    let q = Q as u64;
    assert_eq!(
        spans,
        vec![
            (0, 2 * q, false, false),
            (2 * q, 4 * q, false, true),
            (4 * q, 6 * q, true, false),
        ]
    );
}

#[test]
fn very_high_notes_get_8va() {
    // D7 is far above the treble staff; C5 is not
    let right = track(&[(0, Q, 98), (Q, Q, 100), (2 * Q, 2 * Q, 72)]);
    let score = score(four_four(), vec![right]);
    let ottava: Vec<i8> = voice(&score, 0, 0, 0)
        .events
        .iter()
        .map(|e| e.ottava)
        .collect();
    assert_eq!(ottava, vec![1, 1, 0]);
}

#[test]
fn performance_timing_is_not_readable() {
    let notes: Vec<(u32, u32, u8)> = (0..40)
        .map(|i| (i * 157 + 13, 100, 60 + (i % 12) as u8))
        .collect();
    let score = score(meta(None, None), vec![track(&notes)]);
    assert!(!score.is_readable(), "alignment {}", score.grid_alignment);
}
