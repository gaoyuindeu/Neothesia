//! Measures of a MusicXML score whose length differs from their time signature.
use midi_file::MidiFile;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let file = MidiFile::new(&path).unwrap();
    let score = file.score.as_ref().unwrap();
    let ppq = score.ppq as u64;
    for m in &score.measures {
        let (n, d) = m.time_signature;
        let nominal = n as u64 * 4 * ppq / d as u64;
        let len = m.end_tick - m.start_tick;
        if len != nominal || m.repeat_start || m.repeat_end || m.ending.is_some() {
            println!(
                "unrolled {} (number {}) at quarter {:.2}: len {:.3} q, nominal {:.3} q {}{}{}",
                m.index,
                m.number,
                m.start_tick as f64 / ppq as f64,
                len as f64 / ppq as f64,
                nominal as f64 / ppq as f64,
                if m.repeat_start { " |:" } else { "" },
                if m.repeat_end { " :|" } else { "" },
                m.ending.as_deref().unwrap_or("")
            );
        }
    }
}
