//! List notes that are off the 16th / triplet grid, for tuning the analysis.
use midi_file::MidiFile;

fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = MidiFile::new(&path).unwrap();
    let ppq = file.ppq as i64;
    let mut notes: Vec<_> = file.tracks.iter().flat_map(|t| t.notes.iter()).collect();
    notes.sort_by_key(|n| n.start_tick);
    let off = |t: i64| {
        let a = (t % (ppq / 4)).min(ppq / 4 - t % (ppq / 4));
        let b = (t % (ppq / 3)).min(ppq / 3 - t % (ppq / 3));
        a.min(b)
    };
    let mut shown = 0;
    for (i, n) in notes.iter().enumerate() {
        let t = n.start_tick as i64;
        if off(t) > ppq / 32 && shown < 25 {
            let next = notes[i + 1..].iter().find(|m| m.start_tick > n.start_tick);
            println!(
                "tick {} (beat+{}) pitch {} len {} next gap {:?} next len {:?}",
                t,
                t % ppq,
                n.note,
                n.end_tick - n.start_tick,
                next.map(|m| m.start_tick as i64 - t),
                next.map(|m| m.end_tick - m.start_tick)
            );
            shown += 1;
        }
    }
}
