//! Evaluate hand classification against files that are already split into two
//! hand tracks: merge both tracks, classify, compare with the original split.
//! Single-track files only report how the notes got split.
//!
//! cargo run --release -p midi-file --example hand_eval -- a.mid b.mid ...

use midi_file::{
    Hand, MidiNote, MidiTrack, hands,
    midly::{Smf, Timing},
    tempo_track::TempoTrack,
};

fn raw_tracks(path: &str) -> Option<Vec<MidiTrack>> {
    let data = std::fs::read(path).ok()?;
    let smf = Smf::parse(&data).ok()?;
    let Timing::Metrical(ppq) = smf.header.timing else {
        return None;
    };
    let tempo_track = TempoTrack::build(&smf.tracks, ppq.as_int());
    Some(
        smf.tracks
            .iter()
            .enumerate()
            .map(|(id, events)| MidiTrack::new(id, id, &tempo_track, events))
            .filter(|t| !t.notes.is_empty() && t.has_other_than_drums)
            .collect(),
    )
}

type Labeled = Vec<(MidiNote, Option<Hand>)>;

fn load(path: &str) -> Option<(String, Labeled)> {
    {
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let Some(tracks) = raw_tracks(path) else {
            println!("{name}: parse error");
            return None;
        };

        let mut notes: Vec<(MidiNote, Option<Hand>)> = match tracks.as_slice() {
            [only] => only.notes.iter().map(|n| (n.clone(), None)).collect(),
            [a, b] => {
                let avg = |t: &MidiTrack| {
                    t.notes.iter().map(|n| n.note as f32).sum::<f32>() / t.notes.len() as f32
                };
                let (l, r) = if avg(a) <= avg(b) { (a, b) } else { (b, a) };
                l.notes
                    .iter()
                    .map(|n| (n.clone(), Some(Hand::Left)))
                    .chain(r.notes.iter().map(|n| (n.clone(), Some(Hand::Right))))
                    .collect()
            }
            other => {
                println!("{name}: {} note tracks, skipped", other.len());
                return None;
            }
        };
        notes.sort_by_key(|(n, _)| (n.start, n.note));
        Some((name, notes))
    }
}

/// Returns (correct, total) over files with ground truth
fn evaluate(files: &[(String, Labeled)], params: &hands::Params, verbose: bool) -> (usize, usize) {
    let mut total = 0;
    let mut total_ok = 0;
    let mut base_ok = 0;

    for (name, notes) in files {
        let only_notes: Vec<MidiNote> = notes.iter().map(|(n, _)| n.clone()).collect();
        let predicted = hands::classify_with(&only_notes, params);

        if notes[0].1.is_none() {
            if verbose {
                let left = predicted.iter().filter(|h| **h == Hand::Left).count();
                println!(
                    "{name}: single track, left={left} right={}",
                    predicted.len() - left
                );
            }
            continue;
        }

        let ok = notes
            .iter()
            .zip(predicted.iter())
            .filter(|((_, truth), pred)| *truth == Some(**pred))
            .count();
        let baseline = notes
            .iter()
            .filter(|(n, truth)| *truth == Some(if n.note < 60 { Hand::Left } else { Hand::Right }))
            .count();

        total += notes.len();
        total_ok += ok;
        base_ok += baseline;
        if verbose {
            println!(
                "{name}: {:.1}%  (middle-C split: {:.1}%)",
                100.0 * ok as f64 / notes.len() as f64,
                100.0 * baseline as f64 / notes.len() as f64,
            );
        }
    }

    if verbose && total > 0 {
        println!(
            "TOTAL: {:.1}%  (middle-C split: {:.1}%)",
            100.0 * total_ok as f64 / total as f64,
            100.0 * base_ok as f64 / total as f64,
        );
    }
    (total_ok, total)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sweep = args.iter().any(|a| a == "--sweep");
    let files: Vec<_> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .filter_map(|p| load(p))
        .collect();

    if !sweep {
        evaluate(&files, &hands::Params::default(), true);
        return;
    }

    let mut results = Vec::new();
    let beam_width = args
        .iter()
        .find_map(|a| a.strip_prefix("--beam="))
        .map_or(16, |b| b.parse().unwrap());
    for max_span in [12, 15] {
        for jump_weight in [0.0, 1.0, 3.0, 10.0] {
            for jump_base in [5.0, 7.0, 12.0] {
                for jump_speed in [30.0, 60.0, 120.0] {
                    for center_follow in [0.2, 0.5] {
                        let params = hands::Params {
                            max_span,
                            jump_weight,
                            jump_base,
                            jump_speed,
                            center_follow,
                            beam_width,
                            ..Default::default()
                        };
                        let (ok, total) = evaluate(&files, &params, false);
                        results.push((ok as f64 / total as f64, params));
                    }
                }
            }
        }
    }
    results.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (acc, params) in results.iter().take(10) {
        println!("{:.2}%  {params:?}", acc * 100.0);
    }
}
