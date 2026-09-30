//! From MIDI notes to rhythm: staff assignment, quantization, articulation and voices.

use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    time::Duration,
};

use midly::MidiMessage;

use crate::{Hand, MidiFile};

#[derive(Debug, Clone)]
pub(super) struct Note {
    pub staff: usize,
    pub hand: Option<Hand>,
    pub pitch: u8,
    pub raw_start: u64,
    pub raw_end: u64,
    /// Quantized
    pub start: u64,
    pub end: u64,
    pub start_time: Duration,
    pub end_time: Duration,
    /// Written out trill folded back into its main note
    pub trill: bool,
}

/// Staff follows the hand, except for notes far into the other hand's range,
/// which would otherwise need a pile of ledger lines
/// Without hand information the whole track goes to one staff by its average pitch.
fn staff_for(hand: Option<Hand>, track_average: f32, pitch: u8) -> usize {
    let upper = match hand {
        Some(Hand::Right) => true,
        Some(Hand::Left) => false,
        None => track_average >= 60.0,
    };
    match upper {
        true if pitch < 53 => 1,
        true => 0,
        false if pitch >= 67 => 0,
        false => 1,
    }
}

/// All notes of the piece, sorted by start, and the fraction of them on a 16th/triplet grid
pub(super) fn collect(file: &MidiFile, ppq: u16) -> (Vec<Note>, f32) {
    let sixteenth = (ppq as f64 / 4.0).max(1.0);
    let triplet = (ppq as f64 / 3.0).max(1.0);
    let tolerance = (ppq as f64 / 32.0).max(1.0);
    let on_grid = |tick: u64| {
        let t = tick as f64;
        let a = (t / sixteenth).round() * sixteenth;
        let b = (t / triplet).round() * triplet;
        (a - t).abs().min((b - t).abs()) <= tolerance
    };

    let mut notes = Vec::new();
    let mut aligned = 0;
    for track in file.tracks.iter() {
        if !track.has_other_than_drums {
            continue;
        }
        let average = track.notes.iter().map(|n| n.note as f32).sum::<f32>()
            / track.notes.len().max(1) as f32;
        for note in track.notes.iter().filter(|n| n.channel != 9) {
            if on_grid(note.start_tick) {
                aligned += 1;
            }
            notes.push(Note {
                staff: staff_for(track.hand, average, note.note),
                hand: track.hand,
                pitch: note.note,
                raw_start: note.start_tick,
                raw_end: note.end_tick.max(note.start_tick + 1),
                start: note.start_tick,
                end: note.end_tick,
                start_time: note.start,
                end_time: note.end,
                trill: false,
            });
        }
    }

    let alignment = if notes.is_empty() {
        0.0
    } else {
        aligned as f32 / notes.len() as f32
    };

    // Rolled chords: onsets a hair apart in one staff are one chord
    notes.sort_by_key(|n| (n.staff, n.raw_start, n.pitch));
    let window = (ppq as u64 / 16).max(1);
    let mut cluster: Option<(usize, u64)> = None;
    for note in notes.iter_mut() {
        match cluster {
            Some((staff, start)) if staff == note.staff && note.raw_start - start <= window => {
                note.raw_start = start;
            }
            _ => cluster = Some((note.staff, note.raw_start)),
        }
    }

    let mut notes = fold_trills(notes, ppq);
    notes.sort_by_key(|n| (n.raw_start, n.pitch));
    (notes, alignment)
}

/// Scores often contain trills written out as fast alternating notes. Fold runs of 6+
/// notes faster than 16ths, alternating between two pitches a step apart, into one
/// trilled note. (Slower alternations, like the start of Für Elise, are real notes.)
/// `notes` must be sorted by (staff, start).
fn fold_trills(notes: Vec<Note>, ppq: u16) -> Vec<Note> {
    let max_gap = (ppq as u64 / 6).max(1);
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    let mut i = 0;
    while i < notes.len() {
        let mut j = i;
        while j + 1 < notes.len() {
            let (a, b) = (&notes[j], &notes[j + 1]);
            let alternates = b.staff == a.staff
                && b.raw_start > a.raw_start
                && b.raw_start - a.raw_start <= max_gap
                && (1..=2).contains(&a.pitch.abs_diff(b.pitch))
                && (j == i || b.pitch == notes[j - 1].pitch);
            if !alternates {
                break;
            }
            j += 1;
        }

        if j - i + 1 >= 6 {
            let run = &notes[i..=j];
            let main = run[0].pitch.min(run[1].pitch);
            out.push(Note {
                pitch: main,
                raw_end: run[run.len() - 1].raw_end,
                end: run[run.len() - 1].end,
                end_time: run[run.len() - 1].end_time,
                trill: true,
                ..run[0].clone()
            });
            i = j + 1;
        } else {
            out.push(notes[i].clone());
            i += 1;
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Meter {
    pub start: u64,
    pub len: u64,
    pub beat: u64,
    pub compound: bool,
    pub num: u8,
    pub den: u8,
    pub key: i8,
}

impl Meter {
    pub fn end(&self) -> u64 {
        self.start + self.len
    }

    pub fn beats(&self) -> u64 {
        (self.len / self.beat).max(1)
    }

    /// Middle of the bar in meters with an even number (4+) of beats
    pub fn half_bar(&self) -> Option<u64> {
        let beats = self.beats();
        (beats >= 4 && beats % 2 == 0).then_some(self.beat * beats / 2)
    }

    /// First level below the beat: eighths in 6/8, half beats otherwise
    pub fn subdivision(&self) -> u64 {
        if self.compound {
            self.beat / 3
        } else {
            self.beat / 2
        }
        .max(1)
    }

    /// Metric weight of a position: bar > half bar > beat > subdivisions
    pub fn strength(&self, tick: u64) -> u32 {
        let r = tick - self.start;
        let sub = self.subdivision();
        if r == 0 {
            1000
        } else if self.half_bar().is_some_and(|h| r % h == 0) {
            500
        } else if r % self.beat == 0 {
            100
        } else if r % sub == 0 {
            50
        } else if r % (sub / 2).max(1) == 0 {
            20
        } else if r % (sub / 4).max(1) == 0 {
            10
        } else {
            1
        }
    }
}

pub(super) fn meters(file: &MidiFile, ppq: u16, last_tick: u64) -> Vec<Meter> {
    let mut meters = Vec::new();
    let mut tick = 0u64;
    let whole = 4 * ppq as u64;

    while tick < last_tick {
        let (num, den) = file
            .time_signatures
            .iter()
            .filter(|ts| ts.tick <= tick)
            .next_back()
            .map_or((4, 4), |ts| (ts.numerator.max(1), ts.denominator.max(1)));
        let key = file
            .key_signatures
            .iter()
            .filter(|ks| ks.tick <= tick)
            .next_back()
            .map_or(0, |ks| ks.sharps.clamp(-7, 7));

        let unit = (whole / den as u64).max(1);
        let len = unit * num as u64;
        let compound = den >= 8 && num % 3 == 0 && num > 3;
        let beat = if compound { unit * 3 } else { unit };

        meters.push(Meter {
            start: tick,
            len,
            beat,
            compound,
            num,
            den,
            key,
        });
        tick += len;
    }

    meters
}

/// A beat written as a tuplet (3 in the time of 2)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TupletBeat {
    pub start: u64,
    pub len: u64,
    /// Grid points in the beat (3 or 6)
    pub divisions: u64,
}

fn meter_at(meters: &[Meter], tick: u64) -> Option<&Meter> {
    let i = meters.partition_point(|m| m.start <= tick);
    meters.get(i.checked_sub(1)?)
}

/// Snap every note to the grid of its beat. Each beat (per staff) picks the simplest
/// grid that fits its onsets: plain subdivisions, or triplets.
/// Returns the beats that ended up as triplets, per staff.
pub(super) fn quantize(notes: &mut [Note], meters: &[Meter], ppq: u16) -> [Vec<TupletBeat>; 2] {
    let tolerance = (ppq as u64 / 16).max(1);
    let fit = (ppq as u64 / 24).max(1);

    let beat_of = |tick: u64| -> Option<(u64, u64, bool)> {
        let m = meter_at(meters, tick + tolerance)?;
        let k = ((tick + tolerance - m.start) / m.beat).min(m.beats() - 1);
        Some((m.start + k * m.beat, m.beat, m.compound))
    };

    let candidates = |beat: u64, compound: bool| -> Vec<(u64, bool)> {
        let list: &[(u64, bool)] = if compound {
            &[(3, false), (6, false), (12, false)]
        } else {
            &[(2, false), (4, false), (3, true), (8, false), (6, true)]
        };
        list.iter()
            .map(|&(div, triplet)| ((beat / div).max(1), triplet))
            .collect()
    };

    let mut tuplets: [Vec<TupletBeat>; 2] = [Vec::new(), Vec::new()];

    for staff in 0..2 {
        // Onsets per beat
        let mut beats: BTreeMap<u64, (u64, bool, Vec<u64>)> = BTreeMap::new();
        for note in notes.iter().filter(|n| n.staff == staff) {
            if let Some((start, len, compound)) = beat_of(note.raw_start) {
                beats
                    .entry(start)
                    .or_insert((len, compound, Vec::new()))
                    .2
                    .push(note.raw_start);
            }
        }

        let mut grids: HashMap<u64, u64> = HashMap::new();
        for (&start, (len, compound, onsets)) in &beats {
            let error = |grid: u64, onset: u64| {
                let r = onset as i64 - start as i64;
                let snapped = (r as f64 / grid as f64).round() as i64 * grid as i64;
                (r - snapped).unsigned_abs()
            };

            // A triplet grid needs real evidence: at least two onsets on the 1/3 or 2/3
            // points of the beat, where no plain subdivision could put them
            let third = (*len / 3).max(1);
            let on_thirds = onsets
                .iter()
                .filter(|&&o| {
                    let r = o as i64 - start as i64;
                    let k = (r as f64 / third as f64).round() as i64;
                    k.rem_euclid(3) != 0 && (r - k * third as i64).unsigned_abs() <= fit
                })
                .count();
            let options: Vec<(u64, bool)> = candidates(*len, *compound)
                .into_iter()
                .filter(|(_, triplet)| !triplet || on_thirds >= 2)
                .collect();
            let chosen = options
                .iter()
                .find(|(grid, _)| onsets.iter().all(|&o| error(*grid, o) <= fit))
                .or_else(|| {
                    options.iter().min_by_key(|(grid, _)| {
                        onsets.iter().map(|&o| error(*grid, o)).sum::<u64>()
                    })
                })
                .copied()
                .unwrap_or(((len / 4).max(1), false));

            grids.insert(start, chosen.0);
            if chosen.1 {
                tuplets[staff].push(TupletBeat {
                    start,
                    len: *len,
                    divisions: len / chosen.0,
                });
            }
        }

        let snap = |tick: u64| -> u64 {
            let Some((start, len, compound)) = beat_of(tick) else {
                return tick;
            };
            let default = if compound { len / 6 } else { len / 4 }.max(1);
            let grid = grids.get(&start).copied().unwrap_or(default);
            let r = tick as i64 - start as i64;
            let snapped = (r as f64 / grid as f64).round() as i64 * grid as i64;
            (start as i64 + snapped).max(0) as u64
        };

        for note in notes.iter_mut().filter(|n| n.staff == staff) {
            note.start = snap(note.raw_start);
            note.end = snap(note.raw_end);
            if note.end <= note.start {
                let len = beat_of(note.start).map_or(ppq as u64 / 4, |(_, len, _)| len / 4);
                note.end = note.start + len.max(1);
            }
        }
    }

    notes.sort_by_key(|n| (n.start, n.pitch));
    tuplets
}

/// Notes written together: same start, same written end
#[derive(Debug, Clone)]
pub(super) struct Span {
    pub start: u64,
    pub end: u64,
    /// Indices into the notes
    pub notes: Vec<usize>,
    pub staccato: bool,
}

/// Split one staff into up to two voices.
///
/// Notes are first given a written length: short gaps before the next onset are
/// articulation (legato is assumed), notes held well past the next onset are sustained.
/// Sustained notes and the notes moving under/over them go to separate voices.
pub(super) fn voices(notes: &[Note], staff: usize, ppq: u16) -> [Vec<Span>; 2] {
    let ppq = ppq as u64;
    let indices: Vec<usize> = (0..notes.len())
        .filter(|&i| notes[i].staff == staff)
        .collect();

    let mut onsets: Vec<u64> = indices.iter().map(|&i| notes[i].start).collect();
    onsets.sort_unstable();
    onsets.dedup();

    let mut groups: BTreeMap<(u64, u64), (Vec<usize>, bool)> = BTreeMap::new();
    for &i in &indices {
        let n = &notes[i];
        // After the last note, the next eighth stands in for the next onset
        let next = onsets
            .get(onsets.partition_point(|&o| o <= n.start))
            .copied()
            .or_else(|| {
                let grid = (ppq / 2).max(1);
                Some(n.end.div_ceil(grid) * grid)
            });

        // Real sounding length relative to the distance to the next onset
        // (the quantized end may hide it)
        let sounding = n.raw_end.saturating_sub(n.raw_start) as f64;
        let ratio = |next: u64| sounding / (next - n.start).max(1) as f64;

        let (end, staccato) = match next {
            // Held well past the next onset: sustained
            Some(next) if n.end >= next + (ppq / 4).max(next - n.start) => (n.end, false),
            Some(next) if n.end >= next => (next, ratio(next) <= 0.6 && next - n.start <= ppq / 2),
            Some(next) => {
                let r = ratio(next);
                let span = next - n.start;
                let gap = next - n.end;
                if r >= 0.8 || gap <= ppq / 8 {
                    // Legato, the gap is just imprecision
                    (next, false)
                } else if (0.3..=0.6).contains(&r) && span <= ppq / 2 {
                    // Clearly detached short note: staccato, not a rest.
                    // (Longer spans are usually written as a note and a rest)
                    (next, true)
                } else {
                    // A written rest
                    (n.end, false)
                }
            }
            None => (n.end, false),
        };

        let entry = groups.entry((n.start, end)).or_insert((Vec::new(), true));
        entry.0.push(i);
        entry.1 &= staccato;
    }

    let mut by_start: BTreeMap<u64, Vec<Span>> = BTreeMap::new();
    for ((start, end), (mut members, staccato)) in groups {
        members.sort_by_key(|&i| notes[i].pitch);
        by_start.entry(start).or_default().push(Span {
            start,
            end,
            notes: members,
            staccato,
        });
    }

    let top = |span: &Span| {
        span.notes
            .iter()
            .map(|&i| notes[i].pitch)
            .max()
            .unwrap_or(0)
    };

    let mut voices: [Vec<Span>; 2] = [Vec::new(), Vec::new()];
    let mut free_at = [0u64; 2];
    for (t, mut group) in by_start {
        group.sort_by_key(|s| Reverse(top(s)));
        // At most two parts at once; fold the rest into the lower one
        while group.len() > 2 {
            let extra = group.pop().unwrap();
            let last = group.last_mut().unwrap();
            last.notes.extend(extra.notes);
            last.notes.sort_by_key(|&i| notes[i].pitch);
            last.end = last.end.min(extra.end);
            last.staccato &= extra.staccato;
        }

        let parts = group.len();
        for (i, span) in group.into_iter().enumerate() {
            let preferred = if parts == 2 {
                i
            } else if free_at[0] <= t {
                0
            } else {
                1
            };
            let voice = if free_at[preferred] <= t {
                preferred
            } else if free_at[1 - preferred] <= t {
                1 - preferred
            } else {
                // Both busy: cut short whichever would be free first
                let v = if free_at[0] <= free_at[1] { 0 } else { 1 };
                if let Some(last) = voices[v].last_mut() {
                    last.end = t;
                }
                v
            };
            free_at[voice] = span.end;
            voices[voice].push(span);
        }
    }

    // Second voice parts that line up exactly with the first voice are just one chord
    let [first, second] = &mut voices;
    second.retain(|span| {
        if let Some(target) = first
            .iter_mut()
            .find(|s| s.start == span.start && s.end == span.end)
        {
            target.notes.extend(span.notes.iter().copied());
            target.notes.sort_by_key(|&i| notes[i].pitch);
            target.staccato &= span.staccato;
            false
        } else {
            true
        }
    });

    voices
}

/// Sustain pedal spans (ticks), quantized to 16ths
pub(super) fn pedal(file: &MidiFile, ppq: u16) -> Vec<(u64, u64)> {
    let grid = (ppq as u64 / 4).max(1);
    let snap = |t: u64| (t + grid / 2) / grid * grid;

    let mut changes: Vec<(u64, bool)> = file
        .tracks
        .iter()
        .flat_map(|t| t.events.iter())
        .filter_map(|e| match e.message {
            MidiMessage::Controller { controller, value } if controller.as_int() == 64 => {
                Some((e.tick, value.as_int() >= 64))
            }
            _ => None,
        })
        .collect();
    changes.sort_unstable();
    // Split tracks carry copies of the same pedal events
    changes.dedup();

    let mut spans = Vec::new();
    let mut down: Option<u64> = None;
    for (tick, pressed) in changes {
        match (pressed, down) {
            (true, None) => down = Some(tick),
            (false, Some(start)) => {
                let (a, b) = (snap(start), snap(tick));
                if b > a {
                    spans.push((a, b));
                }
                down = None;
            }
            _ => {}
        }
    }
    spans
}
