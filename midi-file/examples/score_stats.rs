//! Print how well each file fits staff notation.
//!
//! cargo run --release -p midi-file --example score_stats -- a.mid b.mid ...

use midi_file::{MidiFile, score::Score};

fn main() {
    for path in std::env::args().skip(1) {
        let name = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let Ok(file) = MidiFile::new(&path) else {
            println!("{name}: parse error");
            continue;
        };
        let score = Score::new(&file);
        println!(
            "{name}: ppq={} alignment={:.2} readable={} measures={} time_sigs={:?} keys={:?}",
            file.ppq,
            score.grid_alignment,
            score.is_readable(),
            score.measures.len(),
            file.time_signatures
                .iter()
                .take(3)
                .map(|t| (t.numerator, t.denominator))
                .collect::<Vec<_>>(),
            file.key_signatures
                .iter()
                .take(3)
                .map(|k| k.sharps)
                .collect::<Vec<_>>(),
        );
    }
}
