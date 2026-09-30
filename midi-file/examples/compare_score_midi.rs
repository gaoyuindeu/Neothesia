//! Compare our MusicXML playback with a reference MIDI rendering of the same score
//! (note count, pitch multiset, onsets in beats).
//!
//! cargo run --release -p midi-file --example compare_score_midi -- score.musicxml score.mid

use midi_file::MidiFile;
use std::collections::HashMap;

fn onsets(file: &MidiFile) -> Vec<(f64, u8)> {
    let ppq = file.ppq as f64;
    let mut v: Vec<(f64, u8)> = file
        .tracks
        .iter()
        .flat_map(|t| t.notes.iter())
        .map(|n| (n.start_tick as f64 / ppq, n.note))
        .collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ours = MidiFile::new(&args[0]).unwrap();
    let reference = MidiFile::new(&args[1]).unwrap();
    let a = onsets(&ours);
    let b = onsets(&reference);

    let count = |v: &[(f64, u8)]| {
        let mut m: HashMap<u8, i64> = HashMap::new();
        for (_, p) in v {
            *m.entry(*p).or_default() += 1;
        }
        m
    };
    let (ca, cb) = (count(&a), count(&b));
    let pitch_diff: i64 = (0..128u8)
        .map(|p| (ca.get(&p).unwrap_or(&0) - cb.get(&p).unwrap_or(&0)).abs())
        .sum();

    // Onsets (in quarter notes) matched by pitch within 1/16 quarter
    let mut matched = 0;
    let mut used = vec![false; b.len()];
    for &(t, p) in &a {
        let lo = b.partition_point(|x| x.0 < t - 0.07);
        for j in lo..b.len() {
            if b[j].0 > t + 0.07 {
                break;
            }
            if !used[j] && b[j].1 == p {
                used[j] = true;
                matched += 1;
                break;
            }
        }
    }
    if std::env::var("DIVERGE").is_ok() {
        // First reference onset whose pitch isn't found near the same time in ours
        let mut ours_by: Vec<(f64, u8)> = a.clone();
        ours_by.sort_by(|x, y| x.partial_cmp(y).unwrap());
        for (j, &(t, p)) in b.iter().enumerate() {
            let found = a.iter().any(|&(u, q)| q == p && (u - t).abs() < 0.07);
            if !found {
                println!("first reference note not found: quarter {t:.3} pitch {p} (index {j})");
                let near_ours: Vec<String> = a
                    .iter()
                    .filter(|x| (x.0 - t).abs() < 1.5)
                    .map(|x| format!("{:.2}:{}", x.0, x.1))
                    .collect();
                let near_ref: Vec<String> = b
                    .iter()
                    .filter(|x| (x.0 - t).abs() < 1.5)
                    .map(|x| format!("{:.2}:{}", x.0, x.1))
                    .collect();
                println!(
                    "ours: {}
ref:  {}",
                    near_ours.join(" "),
                    near_ref.join(" ")
                );
                break;
            }
        }
    }
    println!(
        "ours {} notes, reference {} notes, pitch count difference {}, onsets matched {:.1}% (last onset {:.1} vs {:.1} quarters)",
        a.len(),
        b.len(),
        pitch_diff,
        100.0 * matched as f64 / b.len().max(1) as f64,
        a.last().map_or(0.0, |x| x.0),
        b.last().map_or(0.0, |x| x.0)
    );
}
