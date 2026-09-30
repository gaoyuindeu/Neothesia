//! Evaluate score-to-performance alignment against ASAP beat annotations.
//!
//! cargo run --release -p midi-file --example align_eval -- <asap piece folder> [performance.mid ...]
//! (all performances with annotations in the folder when none are given)

use std::{path::Path, time::Instant};

use midi_file::{MidiFile, align::align, musicxml};

fn beats(path: &Path) -> Vec<f64> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split('\t').next()?.parse().ok())
        .collect()
}

/// Tick at a time, by searching the tempo map
fn tick_at(file: &MidiFile, seconds: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, 1e9f64);
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        if file
            .tempo_track
            .pulses_to_duration(mid as u64)
            .as_secs_f64()
            < seconds
        {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(&args[0]);
    let score = musicxml::load(&dir.join("xml_score.musicxml")).unwrap();
    let reference = MidiFile::load_midi(&dir.join("midi_score.mid")).unwrap();
    let score_beats = beats(&dir.join("midi_score_annotations.txt"));
    // Beats as quarter positions, then as times in our score
    let our_beats: Vec<f64> = score_beats
        .iter()
        .map(|&t| {
            let quarters = tick_at(&reference, t) / reference.ppq as f64;
            score
                .tempo_track
                .pulses_to_duration((quarters * score.ppq as f64).round() as u64)
                .as_secs_f64()
        })
        .collect();

    let performances: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.ends_with(".mid") && n != "midi_score.mid")
            .filter(|n| dir.join(n.replace(".mid", "_annotations.txt")).exists())
            .collect();
        v.sort();
        v
    };

    let mut all_errors = Vec::new();
    for name in performances {
        let perf = MidiFile::load_midi(&dir.join(&name)).unwrap();
        let truth = beats(&dir.join(name.replace(".mid", "_annotations.txt")));
        let start = Instant::now();
        let Some(alignment) = align(&perf, &score) else {
            println!("{name}: no alignment");
            continue;
        };
        let elapsed = start.elapsed();
        let n = truth.len().min(our_beats.len());
        let mut errors: Vec<f64> = (0..n)
            .map(|k| (alignment.time_map.map(our_beats[k]) - truth[k]).abs())
            .collect();
        errors.sort_by(|a, b| a.total_cmp(b));
        let within = |t: f64| 100.0 * errors.iter().filter(|&&e| e <= t).count() as f64 / n as f64;
        println!(
            "{name}: matched {:.0}% notes, beats {} (score {}, perf {}), median error {:.0} ms, within 50ms {:.0}%, 100ms {:.0}%, 250ms {:.0}%, took {:?}",
            alignment.matched * 100.0,
            n,
            our_beats.len(),
            truth.len(),
            errors[n / 2] * 1000.0,
            within(0.05),
            within(0.1),
            within(0.25),
            elapsed
        );
        all_errors.extend(errors);
    }
    if !all_errors.is_empty() {
        all_errors.sort_by(|a, b| a.total_cmp(b));
        let n = all_errors.len();
        let within =
            |t: f64| 100.0 * all_errors.iter().filter(|&&e| e <= t).count() as f64 / n as f64;
        println!(
            "ALL: {n} beats, median {:.0} ms, within 50ms {:.0}%, 100ms {:.0}%, 250ms {:.0}%",
            all_errors[n / 2] * 1000.0,
            within(0.05),
            within(0.1),
            within(0.25)
        );
    }
}
