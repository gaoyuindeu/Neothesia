//! Read the fingering of a printed score (PDF or image) into a MusicXML file, as the app's
//! "Fingering from PDF" does (the original is kept next to it as .bak):
//! `import_fingering <score.musicxml> <score.pdf>`
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: import_fingering <score.musicxml> <score.pdf>");
        std::process::exit(2);
    }
    let t = std::time::Instant::now();
    let report = midi_file::musicxml::score_reader::read_into_file(
        std::path::Path::new(&args[0]),
        std::path::Path::new(&args[1]),
    )
    .expect("read fingering");
    println!(
        "{} pages, {} staves, {} heads ({} matched with the score), {} digits ({} attached), \
         {} fingers written ({:.1} s)",
        report.pages,
        report.staves,
        report.heads,
        report.aligned,
        report.digits,
        report.attached,
        report.fingers,
        t.elapsed().as_secs_f64()
    );
}
