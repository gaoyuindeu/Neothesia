//! How well fingering comes back from a printed score.
//!
//! Takes a MusicXML score with fingering (the ground truth) and a PDF of the same music
//! with the fingering printed. The fingering is removed from the score, the PDF is read
//! with Audiveris, the recognized fingering is moved into the stripped score, and the
//! result is compared note by note with the ground truth.
//!
//! cargo run --release -p midi-file --example omr_fingering_eval -- <truth.mxl> <score.pdf> [work dir]
//!
//! The work dir keeps Audiveris' output; when it already holds .mxl files they are reused.

use std::path::{Path, PathBuf};

use midi_file::musicxml::{note_fingerings, omr, read_file, strip_fingering, transfer_fingering};

fn mxl_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mxl")) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: omr_fingering_eval <truth.mxl> <score.pdf> [work dir]");
        std::process::exit(2);
    }
    let truth_path = Path::new(&args[0]);
    let pdf = Path::new(&args[1]);
    let work = args.get(2).map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!(
            "omr_{}",
            pdf.file_stem().unwrap_or_default().to_string_lossy()
        ))
    });

    let truth = read_file(truth_path).expect("ground truth");
    let stripped = strip_fingering(&truth).expect("strip");

    let cached = mxl_files(&work);
    let sources: Vec<String> = if cached.is_empty() {
        let audiveris = omr::find_audiveris(None).expect("Audiveris not found");
        let t = std::time::Instant::now();
        let sources = omr::recognize(&audiveris, pdf, &work).expect("recognition");
        eprintln!("Audiveris: {:.0} s", t.elapsed().as_secs_f64());
        sources
    } else {
        cached.iter().map(|p| read_file(p).unwrap()).collect()
    };

    let (out, report) = transfer_fingering(&stripped, &sources).expect("transfer");

    let truth_notes = note_fingerings(&truth).unwrap();
    let out_notes = note_fingerings(&out).unwrap();
    assert_eq!(
        truth_notes.len(),
        out_notes.len(),
        "stripping changed the notes"
    );

    let recognized_notes: usize = sources
        .iter()
        .map(|s| note_fingerings(s).map(|n| n.len()).unwrap_or(0))
        .sum();

    if std::env::var_os("OMR_EVAL_VERBOSE").is_some() {
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
        "{}: notes {} (recognized {}), fingered {} | digits read {} | written {} | correct {} wrong {} missing {} extra {} | precision {:.1}% recall {:.1}%",
        truth_path.file_name().unwrap().to_string_lossy(),
        truth_notes.len(),
        recognized_notes,
        fingered,
        report.found,
        written,
        correct,
        wrong,
        missing,
        extra,
        100.0 * correct as f64 / written.max(1) as f64,
        100.0 * correct as f64 / fingered.max(1) as f64,
    );
}
