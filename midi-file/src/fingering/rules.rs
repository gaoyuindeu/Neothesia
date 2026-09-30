//! Rule based fingering.
//!
//! Each hand is fingered separately. Notes starting together form a chord; every chord
//! gets candidate finger assignments (fingers ordered like the pitches), and dynamic
//! programming picks the sequence with the lowest total difficulty. The difficulty rules
//! follow Parncutt et al. (1997) as refined by Jacobs (2001): comfortable and practical
//! spans for every pair of fingers, penalties for weak fingers, the thumb or the fifth
//! finger on black keys, thumb passing, 3-4 successions, and for moving a finger to a
//! different key. Fingers given by the score are kept fixed.

use crate::Hand;

/// Semitone distances for a pair of fingers (lower finger, higher finger), right hand:
/// minimum practical, minimum comfortable, minimum relaxed, maximum relaxed,
/// maximum comfortable, maximum practical. Negative: the higher finger crosses under.
const SPANS: [((u8, u8), [i32; 6]); 10] = [
    ((1, 2), [-5, -3, 1, 5, 8, 10]),
    ((1, 3), [-4, -2, 3, 6, 9, 12]),
    ((1, 4), [-3, -1, 5, 9, 12, 14]),
    ((1, 5), [-1, 1, 7, 10, 13, 15]),
    ((2, 3), [1, 1, 1, 2, 3, 5]),
    ((2, 4), [1, 1, 3, 4, 5, 7]),
    ((2, 5), [2, 2, 5, 6, 8, 10]),
    ((3, 4), [1, 1, 1, 2, 2, 4]),
    ((3, 5), [1, 1, 3, 4, 5, 7]),
    ((4, 5), [1, 1, 1, 2, 3, 5]),
];

/// Notes starting within this many seconds form one chord
const CHORD_WINDOW: f64 = 0.03;
/// After a gap this long the hand can move freely
const FREE_GAP: f64 = 0.5;

#[derive(Debug, Clone, Copy)]
pub struct FingerNote {
    /// Seconds
    pub start: f64,
    pub end: f64,
    pub pitch: u8,
    /// Finger given by the score, kept as is
    pub fixed: Option<u8>,
}

fn is_black(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

fn spans(a: u8, b: u8) -> &'static [i32; 6] {
    let key = (a.min(b), a.max(b));
    &SPANS.iter().find(|(k, _)| *k == key).unwrap().1
}

/// Cost of playing `p2` with finger `f2` right after (or together with) `p1` with `f1`,
/// by hand; `d` is measured as if for the right hand
fn pair_cost(f1: u8, p1: u8, f2: u8, p2: u8, hand: Hand, together: bool) -> f32 {
    // Mirror the left hand: its thumb is on the right
    let d = match hand {
        Hand::Right => p2 as i32 - p1 as i32,
        Hand::Left => p1 as i32 - p2 as i32,
    };

    if f1 == f2 {
        return if p1 == p2 && !together {
            0.0
        } else {
            // One finger can't play two keys at once, and moving it breaks legato
            12.0 + d.abs() as f32
        };
    }

    // Distance from the lower-numbered finger to the higher one
    let dist = if f1 < f2 { d } else { -d };
    let [min_prac, min_comf, min_rel, max_rel, max_comf, max_prac] = *spans(f1, f2);
    let thumb = f1 == 1 || f2 == 1;
    let mut cost = 0.0;

    // Stretch
    if dist < min_comf {
        cost += 2.0 * (min_comf - dist) as f32;
    }
    if dist > max_comf {
        cost += 2.0 * (dist - max_comf) as f32;
    }
    // Beyond what a hand can do
    if dist < min_prac || dist > max_prac {
        cost += 10.0 * ((min_prac - dist).max(dist - max_prac)) as f32;
    }
    // Small and large spans, relative to the relaxed hand
    if dist < min_rel {
        cost += (min_rel - dist) as f32 * if thumb { 2.0 } else { 1.0 };
    }
    if dist > max_rel {
        cost += (dist - max_rel) as f32 * if thumb { 1.0 } else { 2.0 };
    }

    if !together {
        // 3 and 4 in succession
        if (f1, f2) == (3, 4) || (f1, f2) == (4, 3) {
            cost += 1.0;
            if (f1 == 3 && !is_black(p1) && is_black(p2))
                || (f2 == 3 && !is_black(p2) && is_black(p1))
            {
                cost += 1.0;
            }
        }
        // Thumb passing: the thumb goes under or a finger over it
        let crossing = dist < 0 && thumb;
        if crossing {
            let (thumb_key, other_key) = if f1 == 1 { (p1, p2) } else { (p2, p1) };
            cost += if !is_black(thumb_key) && is_black(other_key) {
                3.0
            } else {
                1.0
            };
        }
    }
    cost
}

/// White-key index of a pitch (black keys count as the white key below)
fn diatonic(pitch: u8) -> i32 {
    const LETTERS: [i32; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    (pitch / 12) as i32 * 7 + LETTERS[(pitch % 12) as usize]
}

/// Where the thumb would be if the hand covered white keys finger by finger
fn thumb_position(finger: u8, pitch: u8, hand: Hand) -> f32 {
    let offset = finger as i32 - 1;
    (match hand {
        Hand::Right => diatonic(pitch) - offset,
        Hand::Left => diatonic(pitch) + offset,
    }) as f32
}

/// Changing hand position, per white key the thumb moves
const POSITION_WEIGHT: f32 = 0.4;

/// Cost of a finger on a key regardless of neighbours
fn finger_cost(f: u8, pitch: u8, previous: Option<u8>) -> f32 {
    let mut cost = 0.0;
    // Weak fingers; the fifth is natural on the outer notes, so it costs less
    if f == 4 {
        cost += 1.0;
    }
    if f == 5 {
        cost += 0.5;
    }
    if is_black(pitch) {
        let white_before = previous.is_some_and(|p| !is_black(p));
        if f == 1 {
            cost += 1.0 + if white_before { 2.0 } else { 0.0 };
        }
        if f == 5 && white_before {
            cost += 2.0;
        }
    }
    cost
}

/// All ways to finger a chord of `n` notes (sorted by pitch): fingers increase with pitch
/// in the right hand, decrease in the left
fn assignments(n: usize, hand: Hand) -> Vec<Vec<u8>> {
    let n = n.min(5);
    let mut out = Vec::new();
    for mask in 0u8..32 {
        if mask.count_ones() as usize != n {
            continue;
        }
        let mut fingers: Vec<u8> = (1..=5).filter(|f| mask & (1 << (f - 1)) != 0).collect();
        if hand == Hand::Left {
            fingers.reverse();
        }
        out.push(fingers);
    }
    out
}

#[derive(Debug, Clone)]
struct Chord {
    start: f64,
    end: f64,
    /// Indices into the notes, sorted by pitch (at most 5 are fingered)
    notes: Vec<usize>,
}

/// Finger every note of one hand. Returns 1..=5 per note (same order as `notes`).
pub fn fingering(notes: &[FingerNote], hand: Hand) -> Vec<u8> {
    let mut result = vec![0u8; notes.len()];
    if notes.is_empty() {
        return result;
    }

    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|&a, &b| {
        notes[a]
            .start
            .total_cmp(&notes[b].start)
            .then(notes[a].pitch.cmp(&notes[b].pitch))
    });

    let mut chords: Vec<Chord> = Vec::new();
    for &i in &order {
        match chords.last_mut() {
            Some(c) if notes[i].start - c.start <= CHORD_WINDOW => {
                c.notes.push(i);
                c.end = c.end.max(notes[i].end);
            }
            _ => chords.push(Chord {
                start: notes[i].start,
                end: notes[i].end,
                notes: vec![i],
            }),
        }
    }
    for c in chords.iter_mut() {
        c.notes.sort_by_key(|&i| notes[i].pitch);
        c.notes.dedup_by_key(|i| notes[*i].pitch);
    }

    // Candidates per chord, restricted by fixed fingers, with their own cost
    let fingered = |c: &Chord| -> Vec<usize> {
        // More than five notes: finger the outer and the highest/lowest inner ones
        let n = c.notes.len();
        if n <= 5 {
            c.notes.clone()
        } else {
            let mut keep = vec![
                c.notes[0],
                c.notes[1],
                c.notes[n / 2],
                c.notes[n - 2],
                c.notes[n - 1],
            ];
            keep.dedup();
            keep
        }
    };
    let chord_notes: Vec<Vec<usize>> = chords.iter().map(fingered).collect();

    let mut candidates: Vec<Vec<(Vec<u8>, f32)>> = Vec::with_capacity(chords.len());
    for (ci, members) in chord_notes.iter().enumerate() {
        let previous = ci
            .checked_sub(1)
            .and_then(|p| chord_notes[p].last())
            .map(|&i| notes[i].pitch);
        let mut list = Vec::new();
        for a in assignments(members.len(), hand) {
            let fits = members
                .iter()
                .zip(&a)
                .all(|(&i, &f)| notes[i].fixed.is_none_or(|x| x == f));
            if !fits {
                continue;
            }
            let mut cost: f32 = members
                .iter()
                .zip(&a)
                .map(|(&i, &f)| finger_cost(f, notes[i].pitch, previous))
                .sum();
            // Fingers of one chord should fit one hand position
            let base = thumb_position(a[0], notes[members[0]].pitch, hand);
            for k in 1..members.len() {
                cost += POSITION_WEIGHT
                    * (thumb_position(a[k], notes[members[k]].pitch, hand) - base).abs();
            }
            for k in 1..members.len() {
                cost += pair_cost(
                    a[k - 1],
                    notes[members[k - 1]].pitch,
                    a[k],
                    notes[members[k]].pitch,
                    hand,
                    true,
                );
            }
            list.push((a, cost));
        }
        if list.is_empty() {
            // Fixed fingers that contradict each other: ignore them here
            list = assignments(members.len(), hand)
                .into_iter()
                .map(|a| (a, 0.0))
                .collect();
        }
        candidates.push(list);
    }

    // Transition cost between consecutive chords
    let transition = |ca: usize, a: &[u8], cb: usize, b: &[u8]| -> f32 {
        let na = &chord_notes[ca];
        let nb = &chord_notes[cb];
        let gap = chords[cb].start - chords[ca].end.min(chords[cb].start);
        let pitch = |i: usize| notes[i].pitch;

        // Between block chords the whole hand moves; a finger changing key is normal
        let block = na.len() > 1 && nb.len() > 1;
        let step = |fa: u8, pa: u8, fb: u8, pb: u8| -> f32 {
            if block && fa == fb && pa != pb {
                1.0 + 0.3 * pa.abs_diff(pb) as f32
            } else {
                pair_cost(fa, pa, fb, pb, hand, false)
            }
        };

        // Melodic moves of the outer voices
        let mut cost = step(
            a[a.len() - 1],
            pitch(na[na.len() - 1]),
            b[b.len() - 1],
            pitch(nb[nb.len() - 1]),
        );
        if na.len() > 1 || nb.len() > 1 {
            cost += step(a[0], pitch(na[0]), b[0], pitch(nb[0]));
            cost /= 2.0;
        }
        // Hand position changes
        let position = |c: &[usize], f: &[u8]| -> f32 {
            c.iter()
                .zip(f)
                .map(|(&i, &f)| thumb_position(f, pitch(i), hand))
                .sum::<f32>()
                / c.len() as f32
        };
        cost += POSITION_WEIGHT * (position(na, a) - position(nb, b)).abs();

        // A finger moving to another key while it is still needed
        for (&ia, &fa) in na.iter().zip(a).filter(|_| !block) {
            for (&ib, &fb) in nb.iter().zip(b) {
                if fa == fb && pitch(ia) != pitch(ib) {
                    cost += 6.0;
                }
            }
        }
        // After a pause the hand may reposition
        if gap > FREE_GAP {
            cost *= 0.2;
        } else if gap > FREE_GAP / 2.0 {
            cost *= 0.6;
        }
        cost
    };

    // Viterbi
    let mut best: Vec<Vec<(f32, usize)>> = Vec::with_capacity(chords.len());
    best.push(candidates[0].iter().map(|(_, c)| (*c, 0)).collect());
    for ci in 1..chords.len() {
        let row: Vec<(f32, usize)> = candidates[ci]
            .iter()
            .map(|(b, own)| {
                candidates[ci - 1]
                    .iter()
                    .enumerate()
                    .map(|(k, (a, _))| (best[ci - 1][k].0 + transition(ci - 1, a, ci, b), k))
                    .min_by(|x, y| x.0.total_cmp(&y.0))
                    .map(|(c, k)| (c + own, k))
                    .unwrap()
            })
            .collect();
        best.push(row);
    }

    let last = best.len() - 1;
    let mut k = best[last]
        .iter()
        .enumerate()
        .min_by(|x, y| x.1.0.total_cmp(&y.1.0))
        .map(|(k, _)| k)
        .unwrap();
    for ci in (0..chords.len()).rev() {
        let (assignment, _) = &candidates[ci][k];
        let members = &chord_notes[ci];
        for (&i, &f) in members.iter().zip(assignment) {
            result[i] = f;
        }
        // Notes left out of big chords take the finger of their nearest neighbour
        for &i in &chords[ci].notes {
            if result[i] == 0 {
                let nearest = members
                    .iter()
                    .zip(assignment)
                    .min_by_key(|(m, _)| notes[**m].pitch.abs_diff(notes[i].pitch))
                    .map(|(_, f)| *f)
                    .unwrap_or(1);
                result[i] = nearest;
            }
        }
        k = best[ci][k].1;
    }

    // Unison duplicates removed by dedup get the same finger as their twin
    for (i, n) in notes.iter().enumerate() {
        if result[i] == 0 {
            result[i] = notes
                .iter()
                .enumerate()
                .find(|(j, m)| {
                    result[*j] != 0
                        && m.pitch == n.pitch
                        && (m.start - n.start).abs() <= CHORD_WINDOW
                })
                .map_or(1, |(j, _)| result[j]);
        }
    }
    result
}
