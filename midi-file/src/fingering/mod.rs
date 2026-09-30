//! Piano fingering for the notes of a song.
//!
//! Fingers printed in the score are kept; the other notes get estimated fingers, one hand
//! at a time, with the printed ones as fixed points. The estimate comes from [`estimate`],
//! so the model behind it can be swapped without touching the rest.

mod rules;

use std::{collections::HashMap, sync::Arc, time::Duration};

pub use rules::{FingerNote, fingering};

use crate::{Hand, MidiFile, score::Score};

/// The finger for a note: 1 thumb .. 5 little finger
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Finger {
    pub digit: u8,
    /// Written in the score (otherwise estimated)
    pub printed: bool,
}

/// Estimate the fingers of one hand's notes (sorted by start), keeping the fixed ones
pub fn estimate(notes: &[FingerNote], hand: Hand) -> Vec<u8> {
    rules::fingering(notes, hand)
}

/// First finger digit of a MusicXML fingering text ("3", "3-1" substitution, ...)
pub fn printed_digit(text: &str) -> Option<u8> {
    text.chars()
        .filter_map(|c| c.to_digit(10))
        .find(|d| (1..=5).contains(d))
        .map(|d| d as u8)
}

/// Notes are matched between the score and the playback by pitch and start time
fn key(pitch: u8, start: Duration) -> (u8, i64) {
    (pitch, (start.as_secs_f64() * 1000.0).round() as i64)
}

fn lookup<T: Copy>(map: &HashMap<(u8, i64), T>, pitch: u8, start: Duration) -> Option<T> {
    let (pitch, ms) = key(pitch, start);
    [ms, ms - 1, ms + 1]
        .into_iter()
        .find_map(|ms| map.get(&(pitch, ms)).copied())
}

/// Every note of the score, in order (chords and their grace notes)
fn score_notes_mut(score: &mut Score) -> impl Iterator<Item = &mut crate::score::ScoreNote> {
    score
        .measures
        .iter_mut()
        .flat_map(|m| m.staves.iter_mut())
        .flat_map(|s| s.voices.iter_mut())
        .flat_map(|v| v.events.iter_mut())
        .flat_map(|e| e.grace.iter_mut().chain(e.notes.iter_mut()))
}

fn printed_fingers(score: &Score) -> HashMap<(u8, i64), u8> {
    let mut map = HashMap::new();
    for m in &score.measures {
        for staff in &m.staves {
            for voice in &staff.voices {
                for event in &voice.events {
                    for note in event.grace.iter().chain(event.notes.iter()) {
                        if note.fingering_auto || note.tie_from_prev {
                            continue;
                        }
                        if let Some(d) = note.fingering.as_deref().and_then(printed_digit) {
                            map.insert(key(note.pitch, note.start), d);
                        }
                    }
                }
            }
        }
    }
    map
}

/// Give every note of the two hands a finger (`MidiNote::finger`). Tracks without a hand
/// are left alone. The score, if the file has one, gets the estimated fingers too.
pub fn annotate(file: &mut MidiFile) {
    let printed = file
        .score
        .as_deref()
        .map(printed_fingers)
        .unwrap_or_default();

    let mut tracks = file.tracks.to_vec();
    for hand in [Hand::Left, Hand::Right] {
        let mut refs: Vec<(usize, usize)> = tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.hand == Some(hand))
            .flat_map(|(ti, t)| (0..t.notes.len()).map(move |ni| (ti, ni)))
            .collect();
        if refs.is_empty() {
            continue;
        }
        refs.sort_by_key(|&(ti, ni)| {
            let n = &tracks[ti].notes[ni];
            (n.start, n.note)
        });

        let notes: Vec<FingerNote> = refs
            .iter()
            .map(|&(ti, ni)| {
                let n = &tracks[ti].notes[ni];
                FingerNote {
                    start: n.start.as_secs_f64(),
                    end: n.end.as_secs_f64(),
                    pitch: n.note,
                    fixed: lookup(&printed, n.note, n.start),
                }
            })
            .collect();
        let digits = estimate(&notes, hand);

        let mut changed: HashMap<usize, Vec<crate::MidiNote>> = HashMap::new();
        for ((ti, ni), (note, digit)) in refs.into_iter().zip(notes.iter().zip(digits)) {
            let list = changed
                .entry(ti)
                .or_insert_with(|| tracks[ti].notes.to_vec());
            list[ni].finger = Some(Finger {
                digit,
                printed: note.fixed.is_some(),
            });
        }
        for (ti, notes) in changed {
            tracks[ti].notes = notes.into();
        }
    }
    file.tracks = tracks.into();

    if let Some(score) = file.score.take() {
        let mut score = Arc::unwrap_or_clone(score);
        fill_score(&mut score, file);
        file.score = Some(Arc::new(score));
    }
}

/// Write the estimated fingers of `file` into the score notes that have none
pub fn fill_score(score: &mut Score, file: &MidiFile) {
    let mut fingers = HashMap::new();
    for track in file.tracks.iter() {
        for note in track.notes.iter() {
            if let Some(finger) = note.finger {
                fingers.insert(key(note.note, note.start), finger);
            }
        }
    }
    if fingers.is_empty() {
        return;
    }
    for note in score_notes_mut(score) {
        if note.fingering.is_some() || note.tie_from_prev {
            continue;
        }
        if let Some(finger) = lookup(&fingers, note.pitch, note.start)
            && !finger.printed
        {
            note.fingering = Some(finger.digit.to_string());
            note.fingering_auto = true;
        }
    }
}

/// Keep only the fingering the display settings ask for
pub fn retain(score: &mut Score, printed: bool, estimated: bool) {
    for note in score_notes_mut(score) {
        let keep = if note.fingering_auto {
            estimated
        } else {
            printed
        };
        if !keep {
            note.fingering = None;
        }
    }
}
