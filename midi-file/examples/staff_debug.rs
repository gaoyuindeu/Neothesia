//! Draw the staves found on the pages of a PDF or image.
//!
//! cargo run --release -p midi-file --example staff_debug -- <score.pdf> <out dir>

use midi_file::musicxml::score_reader::{page, staves};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let pages = page::load(std::path::Path::new(&args[0])).expect("pages");
    std::fs::create_dir_all(&args[1]).unwrap();
    for (n, gray) in pages.iter().enumerate() {
        let t = std::time::Instant::now();
        let bits = page::binarize(gray);
        let angle = page::skew(&bits);
        let bits = page::rotate(&bits, angle);
        let Some((interline, thickness)) = staves::spacing(&bits) else {
            println!("page {n}: no staff spacing");
            continue;
        };
        let found = staves::find(&bits, interline, thickness);
        let systems = staves::systems(&found, 2);
        for st in found.iter().take(2) {
            println!("  staff lines {:?} x {}..{}", st.lines, st.x0, st.x1);
        }
        println!(
            "page {n}: {}x{} skew {angle:.1} interline {interline} thickness {thickness}: {} staves, {} systems ({:.1} s)",
            gray.w,
            gray.h,
            found.len(),
            systems.len(),
            t.elapsed().as_secs_f32()
        );
        let mut img = image::RgbImage::from_fn(bits.w as u32, bits.h as u32, |x, y| {
            if bits.get(x as usize, y as usize) {
                image::Rgb([0, 0, 0])
            } else {
                image::Rgb([255, 255, 255])
            }
        });
        for (si, system) in systems.iter().enumerate() {
            let color = if si % 2 == 0 {
                [255, 0, 0]
            } else {
                [0, 120, 255]
            };
            for &s in system {
                let st = &found[s];
                for &y in &st.lines {
                    for x in st.x0..=st.x1.min(bits.w - 1) {
                        img.put_pixel(x as u32, y.round() as u32, image::Rgb(color));
                    }
                }
            }
        }
        img.save(format!("{}/page{n}.png", args[1])).unwrap();
    }
}
