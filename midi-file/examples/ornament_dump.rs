//! Print the played notes of a MusicXML file between two times (seconds), to check how
//! ornaments are realized: `ornament_dump <file.musicxml> <from> <to>`
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let file = midi_file::musicxml::load(std::path::Path::new(&args[1])).unwrap();
    let from: f64 = args[2].parse().unwrap();
    let to: f64 = args[3].parse().unwrap();
    let mut notes: Vec<_> = file
        .tracks
        .iter()
        .flat_map(|t| t.notes.iter())
        .filter(|n| (from..to).contains(&n.start.as_secs_f64()))
        .collect();
    notes.sort_by_key(|n| (n.start, n.note));
    for n in notes {
        println!(
            "{:8.3}  {:6.3}  track {}  note {}",
            n.start.as_secs_f64(),
            n.duration.as_secs_f64(),
            n.track_id,
            n.note
        );
    }
}
