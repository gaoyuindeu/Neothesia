use midi_file::{
    MidiFile,
    midly::{
        Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
        num::{u4, u7, u15, u28},
    },
    score::{Accidental, NoteValue, Score, StaffItem},
};

const PPQ: u32 = 480;

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

/// Notes as (start, length, key) in ticks, turned into one track
fn track(notes: &[(u32, u32, u8)]) -> Vec<TrackEvent<'static>> {
    let mut points: Vec<(u32, bool, u8)> = Vec::new();
    for &(start, len, key) in notes {
        points.push((start, true, key));
        points.push((start + len, false, key));
    }
    // Note offs before note ons at the same tick
    points.sort_by_key(|(t, on, key)| (*t, *on, *key));

    let mut events = Vec::new();
    let mut last = 0;
    for (t, on, key) in points {
        let message = if on {
            MidiMessage::NoteOn {
                key: u7::new(key),
                vel: u7::new(80),
            }
        } else {
            MidiMessage::NoteOff {
                key: u7::new(key),
                vel: u7::new(0),
            }
        };
        events.push(ev(t - last, midi(message)));
        last = t;
    }
    events.push(ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)));
    events
}

fn file(meta: Vec<TrackEvent<'static>>, tracks: Vec<Vec<TrackEvent<'static>>>) -> MidiFile {
    let mut all = vec![meta];
    all.extend(tracks);
    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(PPQ as u16))),
        tracks: all,
    };
    MidiFile::from_smf("test", &smf).unwrap()
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

fn chords(items: &[StaffItem]) -> Vec<(u64, NoteValue, bool, Vec<u8>)> {
    items
        .iter()
        .filter_map(|i| match i {
            StaffItem::Chord(c) => Some((
                c.tick,
                c.value,
                c.dotted,
                c.notes.iter().map(|n| n.pitch).collect(),
            )),
            _ => None,
        })
        .collect()
}

fn rests(items: &[StaffItem]) -> Vec<(u64, NoteValue, bool)> {
    items
        .iter()
        .filter_map(|i| match i {
            StaffItem::Rest(r) => Some((r.tick, r.value, r.dotted)),
            _ => None,
        })
        .collect()
}

#[test]
fn quarters_chords_and_rests() {
    let q = PPQ;
    // Right hand: C5 quarter, (rest), E5+G5 half. Left hand: C3 whole
    let right = track(&[(0, q, 72), (2 * q, 2 * q, 76), (2 * q, 2 * q, 79)]);
    let left = track(&[(0, 4 * q, 48)]);
    let score = Score::new(&file(meta(Some((4, 2)), None), vec![right, left]));

    assert_eq!(score.measures.len(), 1);
    assert!(score.is_readable());
    let m = &score.measures[0];
    let q = q as u64;

    assert_eq!(
        chords(&m.staves[0]),
        vec![
            (0, NoteValue::Quarter, false, vec![72]),
            (2 * q, NoteValue::Half, false, vec![76, 79]),
        ]
    );
    assert_eq!(rests(&m.staves[0]), vec![(q, NoteValue::Quarter, false)]);
    assert_eq!(
        chords(&m.staves[1]),
        vec![(0, NoteValue::Whole, false, vec![48])]
    );
}

#[test]
fn time_signature_and_empty_staff() {
    let q = PPQ;
    // 3/4, two measures of right hand only
    let right = track(&[(0, 3 * q, 72), (3 * q, 3 * q, 74)]);
    let score = Score::new(&file(meta(Some((3, 2)), None), vec![right]));

    assert_eq!(score.measures.len(), 2);
    assert_eq!(score.measures[1].start_tick, 3 * q as u64);
    assert_eq!(
        chords(&score.measures[0].staves[0]),
        vec![(0, NoteValue::Half, true, vec![72])]
    );
    // Bass staff gets a whole measure rest
    match &score.measures[0].staves[1][..] {
        [StaffItem::Rest(r)] => assert!(r.whole_measure),
        other => panic!("{other:?}"),
    }
}

#[test]
fn accidentals_follow_key_and_measure() {
    let e = PPQ / 2;
    // G major (1 sharp): F#5 needs nothing, F5 needs a natural, the next F#5 a sharp,
    // then C#5 a sharp and the second C#5 nothing
    let right = track(&[
        (0, e, 78),
        (e, e, 77),
        (2 * e, e, 78),
        (3 * e, e, 73),
        (4 * e, e, 73),
    ]);
    let score = Score::new(&file(meta(Some((4, 2)), Some(1)), vec![right]));

    let accidentals: Vec<Option<Accidental>> = score.measures[0].staves[0]
        .iter()
        .filter_map(|i| match i {
            StaffItem::Chord(c) => Some(c.notes[0].accidental),
            _ => None,
        })
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
    let q = PPQ;
    // F major (1 flat): Bb4 is in the key, Eb5 needs a flat
    let right = track(&[(0, q, 70), (q, q, 75)]);
    let score = Score::new(&file(meta(Some((4, 2)), Some(-1)), vec![right]));
    let notes: Vec<(i32, Option<Accidental>)> = score.measures[0].staves[0]
        .iter()
        .filter_map(|i| match i {
            StaffItem::Chord(c) => Some((c.notes[0].step, c.notes[0].accidental)),
            _ => None,
        })
        .collect();
    // B4 = 4*7+6, E5 = 5*7+2
    assert_eq!(notes, vec![(34, None), (37, Some(Accidental::Flat))]);
}

#[test]
fn note_crossing_barline_continues_tied() {
    let q = PPQ;
    let right = track(&[(3 * q, 2 * q, 72), (5 * q, 3 * q, 74)]);
    let score = Score::new(&file(meta(Some((4, 2)), None), vec![right]));

    let second = &score.measures[1].staves[0];
    match &second[0] {
        StaffItem::Chord(c) => {
            assert_eq!(c.tick, 4 * q as u64);
            assert!(c.notes[0].tied);
            assert_eq!(c.value, NoteValue::Quarter);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn performance_timing_is_not_readable() {
    // Onsets scattered off the grid, like a recorded performance
    let notes: Vec<(u32, u32, u8)> = (0..40)
        .map(|i| (i * 157 + 13, 100, 60 + (i % 12) as u8))
        .collect();
    let score = Score::new(&file(meta(None, None), vec![track(&notes)]));
    assert!(!score.is_readable(), "alignment {}", score.grid_alignment);
}
