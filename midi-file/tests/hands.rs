use midi_file::{
    Hand, MidiFile,
    midly::{
        Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind,
        num::{u4, u7, u15, u28},
    },
};

fn note(delta: u32, key: u8, on: bool) -> TrackEvent<'static> {
    let key = u7::new(key);
    let message = if on {
        MidiMessage::NoteOn {
            key,
            vel: u7::new(80),
        }
    } else {
        MidiMessage::NoteOff {
            key,
            vel: u7::new(0),
        }
    };
    TrackEvent {
        delta: u28::new(delta),
        kind: TrackEventKind::Midi {
            channel: u4::new(0),
            message,
        },
    }
}

fn pedal(delta: u32, value: u8) -> TrackEvent<'static> {
    TrackEvent {
        delta: u28::new(delta),
        kind: TrackEventKind::Midi {
            channel: u4::new(0),
            message: MidiMessage::Controller {
                controller: u7::new(64),
                value: u7::new(value),
            },
        },
    }
}

fn end() -> TrackEvent<'static> {
    TrackEvent {
        delta: u28::new(0),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    }
}

fn smf(tracks: Vec<Vec<TrackEvent<'static>>>) -> Smf<'static> {
    Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(480))),
        tracks,
    }
}

/// Bass line C2 E2 G2 under a melody C5 D5 E5, played together on one track
fn single_track() -> Vec<TrackEvent<'static>> {
    let mut events = vec![pedal(0, 127)];
    for (bass, melody) in [(36, 72), (40, 74), (43, 76)] {
        events.push(note(0, bass, true));
        events.push(note(0, melody, true));
        events.push(note(480, bass, false));
        events.push(note(0, melody, false));
    }
    events.push(pedal(0, 0));
    events.push(end());
    events
}

#[test]
fn single_track_is_split_into_two_hands() {
    let meta = vec![end()];
    let file = MidiFile::from_smf("test", &smf(vec![meta, single_track()])).unwrap();

    assert_eq!(file.tracks.len(), 3);
    for (i, track) in file.tracks.iter().enumerate() {
        assert_eq!(track.track_id, i);
        assert!(track.notes.iter().all(|n| n.track_id == i));
        assert!(track.events.iter().all(|e| e.track_id == i));
        assert!(
            track
                .notes
                .iter()
                .all(|n| n.track_color_id == track.track_color_id)
        );
    }

    let left = &file.tracks[1];
    let right = &file.tracks[2];
    assert_eq!(left.hand, Some(Hand::Left));
    assert_eq!(right.hand, Some(Hand::Right));
    assert_eq!(left.track_color_id, Hand::Left.color_id());
    assert_eq!(right.track_color_id, Hand::Right.color_id());

    let pitches = |t: &midi_file::MidiTrack| t.notes.iter().map(|n| n.note).collect::<Vec<_>>();
    assert_eq!(pitches(left), vec![36, 40, 43]);
    assert_eq!(pitches(right), vec![72, 74, 76]);

    for track in [left, right] {
        let ons = track
            .events
            .iter()
            .filter(|e| matches!(e.message, MidiMessage::NoteOn { .. }))
            .count();
        let offs = track
            .events
            .iter()
            .filter(|e| matches!(e.message, MidiMessage::NoteOff { .. }))
            .count();
        let pedals = track
            .events
            .iter()
            .filter(|e| matches!(e.message, MidiMessage::Controller { .. }))
            .count();
        assert_eq!((ons, offs, pedals), (3, 3, 2));
    }
}

#[test]
fn two_tracks_are_colored_by_pitch() {
    let high = vec![note(0, 72, true), note(480, 72, false), end()];
    let low = vec![note(0, 36, true), note(480, 36, false), end()];
    let file = MidiFile::from_smf("test", &smf(vec![high, low])).unwrap();

    assert_eq!(file.tracks.len(), 2);
    assert_eq!(file.tracks[0].hand, Some(Hand::Right));
    assert_eq!(file.tracks[1].hand, Some(Hand::Left));
    assert_eq!(
        file.tracks[1].notes[0].track_color_id,
        Hand::Left.color_id()
    );
}
