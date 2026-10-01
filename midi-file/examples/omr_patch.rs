//! Read the fingering digits of an Audiveris project and write the project with them
//! attached to their notes.
//!
//! cargo run --release -p midi-file --example omr_patch -- <book.omr> <patched.omr>

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: omr_patch <book.omr> <patched.omr>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("read project");
    let t = std::time::Instant::now();
    let (patched, report) =
        midi_file::musicxml::omr_fingering::patch_omr(&bytes).expect("read digits");
    std::fs::write(&args[1], patched).expect("write project");
    println!(
        "{} digits, {} attached ({:.1} s)",
        report.digits,
        report.attached,
        t.elapsed().as_secs_f64()
    );
}
