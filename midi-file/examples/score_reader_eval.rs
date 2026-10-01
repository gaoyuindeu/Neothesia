//! How well the score reader gets fingering back from a printed score.
//!
//! The fingering is removed from a MusicXML score that has it, the printed score (PDF or
//! image) is read with the score reader, and the fingers written back are compared note by
//! note with the original.
//!
//! cargo run --release -p midi-file --example score_reader_eval -- <truth.mxl> <score.pdf>

use std::path::Path;

use midi_file::musicxml::{note_fingerings, read_file, score_reader, strip_fingering};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: score_reader_eval <truth.mxl> <score.pdf>");
        std::process::exit(2);
    }
    let truth_path = Path::new(&args[0]);
    let truth = read_file(truth_path).expect("ground truth");
    let stripped = strip_fingering(&truth).expect("strip");

    let t = std::time::Instant::now();
    let pages = score_reader::page::load(Path::new(&args[1])).expect("pages");
    if std::env::var_os("SCORE_READER_TIMING").is_some() {
        eprintln!("load {:.1?}", t.elapsed());
    }
    let (fingers, report) = score_reader::read(&stripped, &pages).expect("read");
    let out = score_reader::insert_fingers(&stripped, &fingers).expect("insert");
    let seconds = t.elapsed().as_secs_f64();

    let truth_notes = note_fingerings(&truth).unwrap();
    let out_notes = note_fingerings(&out).unwrap();
    assert_eq!(
        truth_notes.len(),
        out_notes.len(),
        "stripping changed the notes"
    );

    if std::env::var_os("SCORE_READER_VERBOSE").is_some() {
        eprintln!("{report:?}");
        for (i, (t, o)) in truth_notes.iter().zip(&out_notes).enumerate() {
            if t.1 != o.1 {
                eprintln!("note {i}: pitch {} truth {:?} got {:?}", t.0, t.1, o.1);
            }
        }
    }

    let (mut fingered, mut correct, mut wrong, mut missing, mut extra) = (0, 0, 0, 0, 0);
    for ((_, t), (_, o)) in truth_notes.iter().zip(&out_notes) {
        match (t, o) {
            (Some(t), Some(o)) if t == o => {
                fingered += 1;
                correct += 1;
            }
            (Some(_), Some(_)) => {
                fingered += 1;
                wrong += 1;
            }
            (Some(_), None) => {
                fingered += 1;
                missing += 1;
            }
            (None, Some(_)) => extra += 1,
            (None, None) => {}
        }
    }
    let written = correct + wrong + extra;
    println!(
        "{}: notes {} (heads found {}, aligned {}), fingered {} | digits read {} | written {} | correct {} wrong {} missing {} extra {} | precision {:.1}% recall {:.1}% ({:.1} s)",
        truth_path.file_name().unwrap().to_string_lossy(),
        truth_notes.len(),
        report.heads,
        report.aligned,
        fingered,
        report.digits,
        written,
        correct,
        wrong,
        missing,
        extra,
        100.0 * correct as f64 / written.max(1) as f64,
        100.0 * correct as f64 / fingered.max(1) as f64,
        seconds
    );
}
