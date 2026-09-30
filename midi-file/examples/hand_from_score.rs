//! Hands taken from an aligned score vs. the hands of a two-track MIDI file.
use midi_file::{Hand, MidiFile};
use std::collections::HashMap;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let truth = MidiFile::load_midi(std::path::Path::new(&path)).unwrap();
    let aligned = MidiFile::new(&path).unwrap();
    let key = |n: &midi_file::MidiNote| (n.start.as_micros(), n.note);
    let truth_hands: HashMap<_, Hand> = truth
        .tracks
        .iter()
        .flat_map(|t| t.notes.iter().map(move |n| (key(n), t.hand)))
        .filter_map(|(k, h)| h.map(|h| (k, h)))
        .collect();
    let (mut ok, mut total) = (0, 0);
    for t in aligned.tracks.iter() {
        for n in t.notes.iter() {
            if let (Some(h), Some(truth)) = (t.hand, truth_hands.get(&key(n))) {
                total += 1;
                ok += (h == *truth) as usize;
            }
        }
    }
    println!(
        "hands from score agree with the file's hand tracks: {:.1}% of {total} notes",
        100.0 * ok as f64 / total as f64
    );
}
