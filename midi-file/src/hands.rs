//! Left / right hand assignment.
//!
//! Single-track piano files (e.g. performance recordings) carry no hand information,
//! so the track is split into two virtual tracks, one per hand. Files with exactly two
//! note tracks are assumed to already be split by hand and only get recolored.

use std::{collections::HashMap, rc::Rc, sync::Arc, time::Duration};

use midly::MidiMessage;

use crate::{MidiEvent, MidiNote, MidiTrack};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hand {
    Left,
    Right,
}

impl Hand {
    /// Index into the color schema (default schema: 1 = blue, 3 = green)
    pub fn color_id(self) -> usize {
        match self {
            Hand::Left => 1,
            Hand::Right => 3,
        }
    }
}

/// Tuning knobs of the hand classifier
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Notes starting within this window are treated as one chord
    pub chord_window: Duration,
    /// A note still counts as held if it sounds at least this long after a new onset
    pub hold_tolerance: Duration,
    /// Comfortable span of one hand, in semitones
    pub max_span: u8,
    pub span_weight: f32,
    pub crossing_weight: f32,
    /// How fast a hand's center follows the notes it plays (0..1)
    pub center_follow: f32,
    /// Penalty per semitone a hand would have to jump faster than it can move
    pub jump_weight: f32,
    /// Distance a hand can travel instantly, in semitones
    pub jump_base: f32,
    /// Hand travel speed, in semitones per second
    pub jump_speed: f32,
    /// Number of hypotheses kept while searching; 1 = greedy
    pub beam_width: usize,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            chord_window: Duration::from_millis(30),
            hold_tolerance: Duration::from_millis(50),
            max_span: 12,
            span_weight: 20.0,
            crossing_weight: 10.0,
            center_follow: 0.2,
            jump_weight: 1.0,
            jump_base: 7.0,
            jump_speed: 60.0,
            beam_width: 16,
        }
    }
}

const MAX_FINGERS: usize = 5;
const DEFAULT_LEFT_CENTER: f32 = 48.0;
const DEFAULT_RIGHT_CENTER: f32 = 72.0;

/// Assign a hand to every note. `notes` must be sorted by start time.
pub fn classify(notes: &[MidiNote]) -> Vec<Hand> {
    classify_with(notes, &Params::default())
}

pub fn classify_with(notes: &[MidiNote], params: &Params) -> Vec<Hand> {
    // Chords: ranges of `notes` indices, each sorted by pitch
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    let mut i = 0;
    while i < notes.len() {
        let cluster_start = notes[i].start;
        let mut j = i;
        while j < notes.len() && notes[j].start <= cluster_start + params.chord_window {
            j += 1;
        }
        let mut cluster: Vec<usize> = (i..j).collect();
        cluster.sort_by_key(|&n| notes[n].note);
        clusters.push(cluster);
        i = j;
    }

    let mut beam = vec![Hypothesis {
        cost: 0.0,
        centers: [DEFAULT_LEFT_CENTER, DEFAULT_RIGHT_CENTER],
        held: [Vec::new(), Vec::new()],
        last: [None, None],
        splits: None,
    }];

    for cluster in &clusters {
        let now = notes[cluster[0]].start;
        let pitches: Vec<u8> = cluster.iter().map(|&n| notes[n].note).collect();

        let mut next = Vec::with_capacity(beam.len() * (pitches.len() + 1));
        for hyp in &beam {
            let held = hyp.held.clone().map(|mut h| {
                h.retain(|(_, end)| *end > now + params.hold_tolerance);
                h
            });

            for k in 0..=pitches.len() {
                let (left, right) = pitches.split_at(k);
                let cost = hyp.cost
                    + hand_cost(left, hyp.centers[0], &held[0], params)
                    + hand_cost(right, hyp.centers[1], &held[1], params)
                    + crossing_cost(left, right, &held, params)
                    + jump_cost(left, &hyp.last[0], now, params)
                    + jump_cost(right, &hyp.last[1], now, params);

                let mut last = hyp.last.clone();
                for (h, part) in [left, right].into_iter().enumerate() {
                    if !part.is_empty() {
                        last[h] = Some((now, part.to_vec()));
                    }
                }

                let mut new_held = held.clone();
                for (pos, &n) in cluster.iter().enumerate() {
                    let hand = if pos < k { 0 } else { 1 };
                    new_held[hand].push((notes[n].note, notes[n].end));
                }

                next.push(Hypothesis {
                    cost,
                    centers: update_centers(hyp.centers, left, right, params),
                    held: new_held,
                    last,
                    splits: Some(Rc::new(SplitNode {
                        split: k,
                        prev: hyp.splits.clone(),
                    })),
                });
            }
        }

        next.sort_by(|a, b| a.cost.total_cmp(&b.cost));
        // Hypotheses that ended up in (almost) the same state are redundant
        let mut kept: Vec<Hypothesis> = Vec::with_capacity(params.beam_width);
        for hyp in next {
            if kept.len() == params.beam_width {
                break;
            }
            let duplicate = kept.iter().any(|k| {
                (k.centers[0] - hyp.centers[0]).abs() < 0.5
                    && (k.centers[1] - hyp.centers[1]).abs() < 0.5
                    && k.held == hyp.held
            });
            if !duplicate {
                kept.push(hyp);
            }
        }
        beam = kept;
    }

    let mut hands = vec![Hand::Right; notes.len()];
    let mut node = beam.into_iter().next().and_then(|h| h.splits);
    for cluster in clusters.iter().rev() {
        let Some(n) = node else { break };
        for (pos, &note) in cluster.iter().enumerate() {
            hands[note] = if pos < n.split {
                Hand::Left
            } else {
                Hand::Right
            };
        }
        node = n.prev.clone();
    }

    hands
}

struct SplitNode {
    split: usize,
    prev: Option<Rc<SplitNode>>,
}

struct Hypothesis {
    cost: f32,
    centers: [f32; 2],
    /// Currently sounding notes per hand: (pitch, end)
    held: [Vec<(u8, Duration)>; 2],
    /// Onset time and pitches of the last chord played by each hand
    last: [Option<(Duration, Vec<u8>)>; 2],
    /// Split index of every cluster so far, newest first
    splits: Option<Rc<SplitNode>>,
}

fn jump_cost(
    pitches: &[u8],
    last: &Option<(Duration, Vec<u8>)>,
    now: Duration,
    params: &Params,
) -> f32 {
    let (Some((time, last_pitches)), false) = (last, pitches.is_empty()) else {
        return 0.0;
    };
    if params.jump_weight == 0.0 {
        return 0.0;
    }

    let lo = *last_pitches.first().unwrap() as f32;
    let hi = *last_pitches.last().unwrap() as f32;
    let distance = pitches
        .iter()
        .map(|&p| {
            let p = p as f32;
            (lo - p).max(p - hi).max(0.0)
        })
        .fold(0.0, f32::max);

    let reach = params.jump_base + params.jump_speed * (now - *time).as_secs_f32();
    params.jump_weight * (distance - reach).max(0.0)
}

fn update_centers(mut centers: [f32; 2], left: &[u8], right: &[u8], params: &Params) -> [f32; 2] {
    for (h, part) in [left, right].into_iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let mean = part.iter().map(|&p| p as f32).sum::<f32>() / part.len() as f32;
        centers[h] += (mean - centers[h]) * params.center_follow;
    }

    // Keep the hands from collapsing onto each other
    if centers[1] - centers[0] < 5.0 {
        let mid = (centers[0] + centers[1]) / 2.0;
        centers = [mid - 2.5, mid + 2.5];
    }
    centers
}

fn hand_cost(pitches: &[u8], center: f32, held: &[(u8, Duration)], params: &Params) -> f32 {
    if pitches.is_empty() {
        return 0.0;
    }

    let mut cost: f32 = pitches.iter().map(|&p| (p as f32 - center).abs()).sum();

    let all = pitches.iter().copied().chain(held.iter().map(|(p, _)| *p));
    let (lo, hi) = all.fold((u8::MAX, 0), |(lo, hi), p| (lo.min(p), hi.max(p)));
    let span = hi - lo;
    if span > params.max_span {
        cost += params.span_weight * (span - params.max_span) as f32;
    }

    let fingers = pitches.len() + held.len();
    if fingers > MAX_FINGERS {
        cost += 50.0 * (fingers - MAX_FINGERS) as f32;
    }

    cost
}

/// Penalize the left hand playing above a note held by the right hand, and vice versa
fn crossing_cost(
    left: &[u8],
    right: &[u8],
    held: &[Vec<(u8, Duration)>; 2],
    params: &Params,
) -> f32 {
    let right_lowest_held = held[1].iter().map(|(p, _)| *p).min();
    let left_highest_held = held[0].iter().map(|(p, _)| *p).max();

    let mut cost = 0.0;
    if let (Some(&l), Some(r)) = (left.last(), right_lowest_held) {
        if l > r {
            cost += params.crossing_weight * (l - r) as f32;
        }
    }
    if let (Some(&r), Some(l)) = (right.first(), left_highest_held) {
        if r < l {
            cost += params.crossing_weight * (l - r) as f32;
        }
    }
    cost
}

fn is_note_track(track: &MidiTrack) -> bool {
    !track.notes.is_empty() && track.has_other_than_drums
}

fn average_pitch(track: &MidiTrack) -> f32 {
    track.notes.iter().map(|n| n.note as f32).sum::<f32>() / track.notes.len().max(1) as f32
}

/// Assign hands to the tracks of a file, splitting a lone piano track in two.
///
/// Track ids are renumbered so that `track.track_id` stays equal to its index.
pub fn assign_hands(tracks: Vec<MidiTrack>) -> Vec<MidiTrack> {
    let note_tracks: Vec<usize> = (0..tracks.len())
        .filter(|&i| is_note_track(&tracks[i]))
        .collect();

    let mut out = Vec::with_capacity(tracks.len() + 1);

    match note_tracks.as_slice() {
        &[only] => {
            for (i, track) in tracks.into_iter().enumerate() {
                if i == only {
                    let (left, right) = split_track(&track);
                    let id = out.len();
                    out.push(retag(left, id, None, Some(Hand::Left)));
                    out.push(retag(right, id + 1, None, Some(Hand::Right)));
                } else {
                    let id = out.len();
                    out.push(retag(track, id, None, None));
                }
            }
        }
        &[a, b] => {
            let (left, right) = if average_pitch(&tracks[a]) <= average_pitch(&tracks[b]) {
                (a, b)
            } else {
                (b, a)
            };
            for (i, track) in tracks.into_iter().enumerate() {
                let hand = if i == left {
                    Some(Hand::Left)
                } else if i == right {
                    Some(Hand::Right)
                } else {
                    None
                };
                let id = out.len();
                out.push(retag(track, id, hand.map(Hand::color_id), hand));
            }
        }
        _ => return tracks,
    }

    out
}

/// Give tracks explicit hands (by track index); tracks with None keep their colors
pub(crate) fn set_hands(tracks: Vec<MidiTrack>, hands: &[Option<Hand>]) -> Vec<MidiTrack> {
    tracks
        .into_iter()
        .enumerate()
        .map(|(i, track)| {
            let hand = hands.get(i).copied().flatten();
            retag(track, i, None, hand)
        })
        .collect()
}

/// Split one track into (left, right) tracks. Non-note events go to both, so that
/// e.g. the sustain pedal still works when one hand is muted.
fn split_track(track: &MidiTrack) -> (MidiTrack, MidiTrack) {
    let mut notes: Vec<MidiNote> = track.notes.to_vec();
    notes.sort_by_key(|n| (n.start, n.note));
    let hands = classify(&notes);
    split_track_with(track, notes, hands)
}

/// Merge all note tracks and split them again with the hand `hand_of` gives each note
/// (notes it doesn't know go by pitch). Other tracks are kept.
pub(crate) fn split_by(
    tracks: Vec<MidiTrack>,
    hand_of: impl Fn(&MidiNote) -> Option<Hand>,
) -> Vec<MidiTrack> {
    let note_tracks: Vec<usize> = (0..tracks.len())
        .filter(|&i| is_note_track(&tracks[i]))
        .collect();
    let Some(&first) = note_tracks.first() else {
        return tracks;
    };

    // One track with everything that sounds
    let mut notes: Vec<MidiNote> = note_tracks
        .iter()
        .flat_map(|&i| tracks[i].notes.iter().cloned())
        .collect();
    notes.sort_by_key(|n| (n.start, n.note));
    let mut events: Vec<MidiEvent> = note_tracks
        .iter()
        .flat_map(|&i| tracks[i].events.iter().cloned())
        .collect();
    events.sort_by_key(|e| e.timestamp);
    let merged = MidiTrack {
        notes: notes.clone().into(),
        events: events.into(),
        ..tracks[first].clone()
    };

    let hands: Vec<Hand> = notes
        .iter()
        .map(|n| hand_of(n).unwrap_or(if n.note < 60 { Hand::Left } else { Hand::Right }))
        .collect();
    let (left, right) = split_track_with(&merged, notes, hands);

    let mut out = Vec::with_capacity(tracks.len() + 1);
    for (i, track) in tracks.into_iter().enumerate() {
        if i == first {
            let id = out.len();
            out.push(retag(left.clone(), id, None, Some(Hand::Left)));
            out.push(retag(right.clone(), id + 1, None, Some(Hand::Right)));
        } else if !note_tracks.contains(&i) {
            let id = out.len();
            out.push(retag(track, id, None, None));
        }
    }
    out
}

/// Split a track by a hand per note (`notes` sorted by start, `hands` in the same order)
fn split_track_with(
    track: &MidiTrack,
    notes: Vec<MidiNote>,
    hands: Vec<Hand>,
) -> (MidiTrack, MidiTrack) {
    let by_onset: HashMap<(u8, Duration), Hand> = notes
        .iter()
        .zip(hands.iter())
        .map(|(n, h)| ((n.note, n.start), *h))
        .collect();

    let fallback = |key: u8| if key < 60 { Hand::Left } else { Hand::Right };

    let mut events: [Vec<MidiEvent>; 2] = [Vec::new(), Vec::new()];
    let mut active: HashMap<u8, Hand> = HashMap::new();
    for event in track.events.iter() {
        match event.message {
            MidiMessage::NoteOn { key, .. } => {
                let key = key.as_int();
                let hand = by_onset
                    .get(&(key, event.timestamp))
                    .copied()
                    .unwrap_or_else(|| fallback(key));
                active.insert(key, hand);
                events[hand as usize].push(event.clone());
            }
            MidiMessage::NoteOff { key, .. } => {
                let key = key.as_int();
                let hand = active.remove(&key).unwrap_or_else(|| fallback(key));
                events[hand as usize].push(event.clone());
            }
            _ => {
                events[0].push(event.clone());
                events[1].push(event.clone());
            }
        }
    }

    let [left_events, right_events] = events;
    let mut split_notes: [Vec<MidiNote>; 2] = [Vec::new(), Vec::new()];
    for (note, hand) in notes.into_iter().zip(hands) {
        split_notes[hand as usize].push(note);
    }
    let [left_notes, right_notes] = split_notes;

    let make = |notes: Vec<MidiNote>, events: Vec<MidiEvent>| MidiTrack {
        notes: notes.into(),
        events: events.into(),
        track_id: track.track_id,
        track_color_id: track.track_color_id,
        programs: track.programs.clone(),
        has_drums: track.has_drums,
        has_other_than_drums: track.has_other_than_drums,
        hand: None,
    };

    (
        make(left_notes, left_events),
        make(right_notes, right_events),
    )
}

/// Set track id, color (hand color if `hand` is given and `color_id` is None) and hand
fn retag(
    track: MidiTrack,
    track_id: usize,
    color_id: Option<usize>,
    hand: Option<Hand>,
) -> MidiTrack {
    let track_color_id = color_id
        .or(hand.map(Hand::color_id))
        .unwrap_or(track.track_color_id);

    let notes: Arc<[MidiNote]> = track
        .notes
        .iter()
        .map(|n| MidiNote {
            track_id,
            track_color_id,
            ..n.clone()
        })
        .collect();
    let events: Arc<[MidiEvent]> = track
        .events
        .iter()
        .map(|e| MidiEvent {
            track_id,
            track_color_id,
            ..e.clone()
        })
        .collect();

    MidiTrack {
        notes,
        events,
        track_id,
        track_color_id,
        hand,
        ..track
    }
}
