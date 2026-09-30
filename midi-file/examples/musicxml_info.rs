//! Load MusicXML files and print what was read.
//!
//! cargo run --release -p midi-file --example musicxml_info -- a.musicxml ...

use midi_file::{MidiFile, score::DirectionKind};

fn main() {
    for path in std::env::args().skip(1) {
        let start = std::time::Instant::now();
        let file = match MidiFile::new(&path) {
            Ok(f) => f,
            Err(e) => {
                println!("{path}: ERROR {e}");
                continue;
            }
        };
        let elapsed = start.elapsed();
        let score = file.score.as_ref().expect("score");
        let notes: usize = file.tracks.iter().map(|t| t.notes.len()).sum();
        let length = file
            .tracks
            .iter()
            .filter_map(|t| t.notes.iter().map(|n| n.end).max())
            .max()
            .unwrap_or_default();
        let events: usize = score
            .measures
            .iter()
            .flat_map(|m| m.staves.iter())
            .flat_map(|s| s.voices.iter())
            .map(|v| v.events.len())
            .sum();
        let voices_max = score
            .measures
            .iter()
            .flat_map(|m| m.staves.iter())
            .map(|s| s.voices.len())
            .max()
            .unwrap_or(0);
        let dynamics = score
            .measures
            .iter()
            .flat_map(|m| m.directions.iter())
            .filter(|d| matches!(d.kind, DirectionKind::Dynamic(_)))
            .count();
        let words = score
            .measures
            .iter()
            .flat_map(|m| m.directions.iter())
            .filter(|d| matches!(d.kind, DirectionKind::Words { .. }))
            .count();
        let clef_changes: usize = score
            .measures
            .iter()
            .flat_map(|m| m.staves.iter())
            .map(|s| s.clef_changes.len())
            .sum();
        let ottava = score
            .measures
            .iter()
            .flat_map(|m| m.staves.iter())
            .flat_map(|s| s.voices.iter())
            .flat_map(|v| v.events.iter())
            .filter(|e| e.ottava != 0)
            .count();
        let hands: Vec<String> = file
            .tracks
            .iter()
            .filter(|t| !t.notes.is_empty())
            .map(|t| format!("{:?}:{}", t.hand, t.notes.len()))
            .collect();
        println!(
            "{}: {:?} ppq {} | {} measures, {} events, max voices {}, {} notes played, {:.0}s | slurs {} wedges {} dynamics {} words {} clef changes {} 8va events {} pedal spans {} | hands {:?}",
            std::path::Path::new(&path)
                .parent()
                .and_then(|p| p.file_name())
                .unwrap()
                .to_string_lossy(),
            elapsed,
            score.ppq,
            score.measures.len(),
            events,
            voices_max,
            notes,
            length.as_secs_f64(),
            score.slurs.len(),
            score.wedges.len(),
            dynamics,
            words,
            clef_changes,
            ottava,
            score.measures.iter().map(|m| m.pedal.len()).sum::<usize>(),
            hands
        );
    }
}
