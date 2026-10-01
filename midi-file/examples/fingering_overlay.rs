//! Development: what the PDF fingering reader saw, for checking by eye. Writes one line
//! per digit read: `page digit x0 y0 x1 y1 attached hx0 hy0 hx1 hy1` (boxes in pixels of
//! the page rendered at 300 dpi; the head box is that of the note the digit went to)
//! `fingering_overlay <score.musicxml> <score.pdf> <out.txt>`
use midi_file::musicxml::score_reader;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("usage: fingering_overlay <score.musicxml> <score.pdf> <out.txt>");
        std::process::exit(2);
    }
    let xml = midi_file::musicxml::read_file(std::path::Path::new(&args[0])).expect("score");
    let pages = score_reader::page::load(std::path::Path::new(&args[1])).expect("pages");
    let (fingers, report, details) = score_reader::read_detailed(&xml, &pages).expect("read");
    let mut out = String::new();
    for (page, digit, b, note) in &details.digit_boxes {
        let head = note.and_then(|n| details.heads.get(&n));
        let h = head.map_or([-1.0; 4], |(_, h)| *h);
        out += &format!(
            "{page} {digit} {:.0} {:.0} {:.0} {:.0} {} {:.0} {:.0} {:.0} {:.0}\n",
            b[0],
            b[1],
            b[2],
            b[3],
            u8::from(note.is_some()),
            h[0],
            h[1],
            h[2],
            h[3]
        );
    }
    std::fs::write(&args[2], out).expect("write");
    println!("{report:?}, {} fingers", fingers.len());
}
