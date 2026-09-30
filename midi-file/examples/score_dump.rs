//! Print the notation of some measures, for debugging.
//!
//! cargo run --release -p midi-file --example score_dump -- file.mid <first> [count]

use midi_file::{MidiFile, score::Score};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = MidiFile::new(&args[0]).unwrap();
    let first: usize = args.get(1).map_or(0, |a| a.parse().unwrap());
    let count: usize = args.get(2).map_or(1, |a| a.parse().unwrap());
    let score = Score::new(&file);

    for m in score.measures.iter().skip(first).take(count) {
        println!(
            "measure {} ticks {}..{} time {:?} key {}",
            m.index + 1,
            m.start_tick,
            m.end_tick,
            m.time_signature,
            m.key
        );
        for (s, staff) in m.staves.iter().enumerate() {
            for (v, voice) in staff.voices.iter().enumerate() {
                let events: Vec<String> = voice
                    .events
                    .iter()
                    .map(|e| {
                        let notes: Vec<String> =
                            e.notes.iter().map(|n| n.pitch.to_string()).collect();
                        format!(
                            "{}:{:?}{}{}{}",
                            e.tick - m.start_tick,
                            e.value,
                            ".".repeat(e.dots as usize),
                            if e.tuplet.is_some() { "(3)" } else { "" },
                            if notes.is_empty() {
                                "R".to_string()
                            } else {
                                format!("[{}]", notes.join(","))
                            }
                        )
                    })
                    .collect();
                println!("  staff {s} voice {v}: {}", events.join(" "));
            }
        }
    }
}
