//! Load files the way the app does and report hands and notation.
use midi_file::MidiFile;
fn main() {
    for path in std::env::args().skip(1) {
        let start = std::time::Instant::now();
        let file = MidiFile::new(&path).unwrap();
        let hands: Vec<String> = file
            .tracks
            .iter()
            .filter(|t| !t.notes.is_empty())
            .map(|t| format!("{:?}:{}", t.hand, t.notes.len()))
            .collect();
        println!(
            "{}: {:?} notation {} ({} measures) hands {:?}",
            std::path::Path::new(&path)
                .file_name()
                .unwrap()
                .to_string_lossy(),
            start.elapsed(),
            if file.score.is_some() {
                "from score"
            } else {
                "from MIDI"
            },
            file.score.as_ref().map_or(0, |s| s.measures.len()),
            hands
        );
    }
}
