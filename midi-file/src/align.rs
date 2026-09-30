//! Aligning a recorded performance with its score.
//!
//! Both are reduced to sequences of chords (notes starting together) and matched with
//! dynamic time warping. The matched chords give a monotonic map from score time to
//! performance time; notes are then paired one by one to light the sheet up in sync
//! with the recording and to take each performed note's hand from its staff.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{Hand, MidiFile, MidiNote, score::Score};

/// Notes starting closer than this form one chord
const SCORE_CHORD: f64 = 0.015;
const PERFORMANCE_CHORD: f64 = 0.035;
/// Extra cost for aligning one chord with several on the other side
const STRETCH_PENALTY: f32 = 0.15;
/// Chords matching worse than this don't anchor the time map
const ANCHOR_COST: f32 = 0.5;
/// A performed note must be this close to where the map expects it
const NOTE_WINDOW: f64 = 0.3;
/// Above this many DTW cells, only a band around the diagonal is searched
const FULL_DTW_CELLS: usize = 60_000_000;

/// Minimum share of score notes found in the performance to accept an alignment
pub const MIN_MATCHED: f32 = 0.6;

#[derive(Debug, Clone)]
struct Chord {
    time: f64,
    /// Sorted pitches (with repeats)
    pitches: Vec<u8>,
}

fn chords(notes: &[(f64, u8)], window: f64) -> Vec<Chord> {
    let mut out: Vec<Chord> = Vec::new();
    for &(t, p) in notes {
        match out.last_mut() {
            Some(c) if t - c.time <= window => c.pitches.push(p),
            _ => out.push(Chord {
                time: t,
                pitches: vec![p],
            }),
        }
    }
    for c in out.iter_mut() {
        c.pitches.sort_unstable();
    }
    out
}

/// 1 - Dice similarity of the pitch multisets
fn cost(a: &Chord, b: &Chord) -> f32 {
    let (mut i, mut j, mut common) = (0, 0, 0);
    while i < a.pitches.len() && j < b.pitches.len() {
        match a.pitches[i].cmp(&b.pitches[j]) {
            std::cmp::Ordering::Equal => {
                common += 1;
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
        }
    }
    1.0 - 2.0 * common as f32 / (a.pitches.len() + b.pitches.len()) as f32
}

/// DTW path as (score chord, performance chord) pairs that step diagonally
fn dtw(score: &[Chord], performance: &[Chord]) -> Vec<(usize, usize, f32)> {
    let (n, m) = (score.len(), performance.len());
    if n == 0 || m == 0 {
        return Vec::new();
    }

    // Search a band around the diagonal for very long pieces
    let band = if n * m <= FULL_DTW_CELLS {
        m
    } else {
        ((FULL_DTW_CELLS / n) / 2).max(200)
    };
    let range = |i: usize| -> (usize, usize) {
        if band >= m {
            return (0, m);
        }
        let center = i * m / n;
        (center.saturating_sub(band), (center + band + 1).min(m))
    };

    // Directions: 0 diagonal, 1 from above (score advances), 2 from the left
    let mut dirs: Vec<Vec<u8>> = Vec::with_capacity(n);
    let mut offsets: Vec<usize> = Vec::with_capacity(n);
    let mut prev_row: Vec<f32> = Vec::new();
    let mut prev_lo = 0;

    for i in 0..n {
        let (lo, hi) = range(i);
        let mut row = vec![f32::INFINITY; hi - lo];
        let mut dir = vec![0u8; hi - lo];
        let get_prev = |j: usize| -> f32 {
            if i == 0 {
                return f32::INFINITY;
            }
            if j >= prev_lo && j - prev_lo < prev_row.len() {
                prev_row[j - prev_lo]
            } else {
                f32::INFINITY
            }
        };
        for j in lo..hi {
            let c = cost(&score[i], &performance[j]);
            let (best, d) = if i == 0 && j == 0 {
                (0.0, 0)
            } else {
                let diag = if j > 0 {
                    get_prev(j - 1)
                } else {
                    f32::INFINITY
                };
                let up = get_prev(j) + STRETCH_PENALTY;
                let left = if j > lo {
                    row[j - lo - 1] + STRETCH_PENALTY
                } else {
                    f32::INFINITY
                };
                if diag <= up && diag <= left {
                    (diag, 0)
                } else if up <= left {
                    (up, 1)
                } else {
                    (left, 2)
                }
            };
            row[j - lo] = best + c;
            dir[j - lo] = d;
        }
        dirs.push(dir);
        offsets.push(lo);
        prev_row = row;
        prev_lo = lo;
    }

    // Backtrack from the end
    let mut path = Vec::new();
    let (mut i, mut j) = (n - 1, m - 1);
    loop {
        let lo = offsets[i];
        if j < lo || j - lo >= dirs[i].len() {
            break;
        }
        let d = dirs[i][j - lo];
        if d == 0 {
            path.push((i, j, cost(&score[i], &performance[j])));
        }
        if i == 0 && j == 0 {
            break;
        }
        match d {
            0 if i > 0 && j > 0 => {
                i -= 1;
                j -= 1;
            }
            1 if i > 0 => i -= 1,
            2 if j > 0 => j -= 1,
            _ => break,
        }
    }
    path.reverse();
    path
}

/// Monotonic map from score time to performance time (seconds)
#[derive(Debug, Clone)]
pub struct TimeMap {
    anchors: Vec<(f64, f64)>,
}

impl TimeMap {
    fn new(mut anchors: Vec<(f64, f64)>) -> Option<Self> {
        anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
        // Strictly increasing on both sides
        let mut clean: Vec<(f64, f64)> = Vec::with_capacity(anchors.len());
        for (s, p) in anchors {
            match clean.last() {
                Some(&(ls, lp)) if s <= ls || p <= lp => {}
                _ => clean.push((s, p)),
            }
        }
        (clean.len() >= 2).then_some(Self { anchors: clean })
    }

    pub fn map(&self, score: f64) -> f64 {
        let a = &self.anchors;
        let i = a.partition_point(|x| x.0 <= score);
        let (p, q) = if i == 0 {
            (a[0], a[1])
        } else if i >= a.len() {
            (a[a.len() - 2], a[a.len() - 1])
        } else {
            (a[i - 1], a[i])
        };
        let slope = (q.1 - p.1) / (q.0 - p.0);
        (p.1 + (score - p.0) * slope).max(0.0)
    }

    pub fn map_duration(&self, score: Duration) -> Duration {
        Duration::from_secs_f64(self.map(score.as_secs_f64()))
    }
}

#[derive(Debug, Clone)]
pub struct Alignment {
    pub time_map: TimeMap,
    /// Share of score notes found in the performance
    pub matched: f32,
    /// Per score note (start in ms, pitch): the performed note's (start, end)
    notes: HashMap<(u64, u8), (Duration, Duration)>,
    /// Hand of each performed note, by (start in µs, pitch)
    hands: HashMap<(u64, u8), Hand>,
}

fn note_key_ms(start: Duration, pitch: u8) -> (u64, u8) {
    ((start.as_secs_f64() * 1000.0).round() as u64, pitch)
}

fn note_key_us(start: Duration, pitch: u8) -> (u64, u8) {
    (start.as_micros() as u64, pitch)
}

fn all_notes(file: &MidiFile) -> Vec<(MidiNote, Option<Hand>)> {
    let mut notes: Vec<(MidiNote, Option<Hand>)> = file
        .tracks
        .iter()
        .filter(|t| t.has_other_than_drums)
        .flat_map(|t| t.notes.iter().map(move |n| (n.clone(), t.hand)))
        .filter(|(n, _)| n.channel != 9)
        .collect();
    notes.sort_by(|a, b| a.0.start.cmp(&b.0.start).then(a.0.note.cmp(&b.0.note)));
    notes
}

/// Align a performance with the playback of a score file
pub fn align(performance: &MidiFile, score: &MidiFile) -> Option<Alignment> {
    let perf = all_notes(performance);
    let score_notes = all_notes(score);
    if perf.is_empty() || score_notes.is_empty() {
        return None;
    }

    let s_onsets: Vec<(f64, u8)> = score_notes
        .iter()
        .map(|(n, _)| (n.start.as_secs_f64(), n.note))
        .collect();
    let p_onsets: Vec<(f64, u8)> = perf
        .iter()
        .map(|(n, _)| (n.start.as_secs_f64(), n.note))
        .collect();
    let s_chords = chords(&s_onsets, SCORE_CHORD);
    let p_chords = chords(&p_onsets, PERFORMANCE_CHORD);

    let path = dtw(&s_chords, &p_chords);
    let anchors: Vec<(f64, f64)> = path
        .iter()
        .filter(|(_, _, c)| *c <= ANCHOR_COST)
        .map(|&(i, j, _)| (s_chords[i].time, p_chords[j].time))
        .collect();
    let time_map = TimeMap::new(anchors)?;

    // Pair notes: same pitch, closest to where the map expects it
    let mut by_pitch: HashMap<u8, Vec<usize>> = HashMap::new();
    for (k, (n, _)) in perf.iter().enumerate() {
        by_pitch.entry(n.note).or_default().push(k);
    }
    let mut used = vec![false; perf.len()];
    let mut notes = HashMap::new();
    let mut hands = HashMap::new();
    let mut matched = 0;
    for (n, hand) in &score_notes {
        let expected = time_map.map(n.start.as_secs_f64());
        let best = by_pitch.get(&n.note).and_then(|list| {
            list.iter()
                .copied()
                .filter(|&k| !used[k])
                .map(|k| (k, (perf[k].0.start.as_secs_f64() - expected).abs()))
                .filter(|&(_, d)| d <= NOTE_WINDOW)
                .min_by(|a, b| a.1.total_cmp(&b.1))
        });
        if let Some((k, _)) = best {
            used[k] = true;
            matched += 1;
            notes.insert(
                note_key_ms(n.start, n.note),
                (perf[k].0.start, perf[k].0.end),
            );
            if let Some(h) = hand {
                hands.insert(note_key_us(perf[k].0.start, perf[k].0.note), *h);
            }
        }
    }

    // Unpaired performed notes take the hand of the nearest paired note in pitch
    let paired: Vec<(f64, u8, Hand)> = perf
        .iter()
        .filter_map(|(n, _)| {
            hands
                .get(&note_key_us(n.start, n.note))
                .map(|h| (n.start.as_secs_f64(), n.note, *h))
        })
        .collect();
    for (n, _) in &perf {
        let key = note_key_us(n.start, n.note);
        if hands.contains_key(&key) {
            continue;
        }
        let t = n.start.as_secs_f64();
        let lo = paired.partition_point(|x| x.0 < t - 0.5);
        let hand = paired[lo..]
            .iter()
            .take_while(|x| x.0 <= t + 0.5)
            .min_by_key(|x| x.1.abs_diff(n.note))
            .map(|x| x.2)
            .unwrap_or(if n.note < 60 { Hand::Left } else { Hand::Right });
        hands.insert(key, hand);
    }

    Some(Alignment {
        time_map,
        matched: matched as f32 / score_notes.len() as f32,
        notes,
        hands,
    })
}

impl Alignment {
    /// The score with its times moved onto the performance
    pub fn warp_score(&self, score: &Score) -> Score {
        let mut out = score.clone();
        let map_note = |start: Duration, end: Duration, pitch: u8| -> (Duration, Duration) {
            self.notes
                .get(&note_key_ms(start, pitch))
                .copied()
                .unwrap_or_else(|| {
                    (
                        self.time_map.map_duration(start),
                        self.time_map.map_duration(end),
                    )
                })
        };
        for m in out.measures.iter_mut() {
            m.start = self.time_map.map_duration(m.start);
            m.end = self.time_map.map_duration(m.end);
            for staff in m.staves.iter_mut() {
                for voice in staff.voices.iter_mut() {
                    for event in voice.events.iter_mut() {
                        for n in event.notes.iter_mut().chain(event.grace.iter_mut()) {
                            (n.start, n.end) = map_note(n.start, n.end, n.pitch);
                        }
                    }
                }
            }
        }
        // Keep measures ordered even where the map is flat
        for i in 1..out.measures.len() {
            if out.measures[i].start < out.measures[i - 1].start {
                out.measures[i].start = out.measures[i - 1].start;
            }
            if out.measures[i - 1].end > out.measures[i].start {
                out.measures[i - 1].end = out.measures[i].start;
            }
        }
        out
    }

    pub fn hand_of(&self, note: &MidiNote) -> Option<Hand> {
        self.hands.get(&note_key_us(note.start, note.note)).copied()
    }
}

/// A MusicXML score next to a MIDI file: same name, or the only score in the folder
pub fn find_score_for(midi: &Path) -> Option<PathBuf> {
    let dir = midi.parent()?;
    let stem = midi.file_stem()?.to_string_lossy().to_lowercase();
    let scores: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| crate::musicxml::is_musicxml(p))
        .collect();
    scores
        .iter()
        .find(|p| {
            p.file_stem()
                .is_some_and(|s| s.to_string_lossy().to_lowercase() == stem)
        })
        .cloned()
        .or_else(|| (scores.len() == 1).then(|| scores[0].clone()))
}

/// Pair a performance with a score: hands and notation come from the score.
/// Returns the performance unchanged when they don't match well enough.
pub fn attach_score(performance: MidiFile, score_file: &MidiFile) -> (MidiFile, Option<f32>) {
    let Some(score) = score_file.score.as_ref() else {
        return (performance, None);
    };
    let Some(alignment) = align(&performance, score_file) else {
        return (performance, None);
    };
    if alignment.matched < MIN_MATCHED {
        return (performance, Some(alignment.matched));
    }

    let mut performance = performance;
    performance.tracks =
        crate::hands::split_by(performance.tracks.to_vec(), |n| alignment.hand_of(n)).into();
    performance.score = Some(Arc::new(alignment.warp_score(score)));
    (performance, Some(alignment.matched))
}
