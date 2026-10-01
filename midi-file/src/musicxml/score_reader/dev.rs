//! Development aid: run the detector outside (on a GPU with PyTorch) on the very pages the
//! reader works on. `SCORE_READER_PAGES_OUT=<dir>` writes each prepared page
//! (`<n>.png`, black and white, and `<n>.json` with its staff line distance);
//! `SCORE_READER_DETECTIONS=<dir>` reads `<n>.det.json` (`[[class, score, x0, y0, x1, y1]]`)
//! instead of running the detector.

use std::path::Path;

use super::{detect::Found, page::Bitmap};

pub fn write_page(dir: &Path, page: usize, bits: &Bitmap, interline: f32) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = std::fs::File::create(dir.join(format!("{page}.png"))).map_err(|e| e.to_string())?;
    let mut encoder =
        png::Encoder::new(std::io::BufWriter::new(file), bits.w as u32, bits.h as u32);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    let data: Vec<u8> = bits.px.iter().map(|&b| if b { 0 } else { 255 }).collect();
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(&data))
        .map_err(|e| e.to_string())?;
    std::fs::write(
        dir.join(format!("{page}.json")),
        format!("{{\"interline\": {interline}}}"),
    )
    .map_err(|e| e.to_string())
}

pub fn read_detections(dir: &Path, page: usize, threshold: f32) -> Result<Vec<Found>, String> {
    let text =
        std::fs::read_to_string(dir.join(format!("{page}.det.json"))).map_err(|e| e.to_string())?;
    // [[class, score, x0, y0, x1, y1], ...]
    let numbers: Vec<f32> = text
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e'))
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect();
    Ok(numbers
        .as_chunks::<6>()
        .0
        .iter()
        .filter(|v| v[1] >= threshold)
        .map(|v| Found {
            class: v[0] as u8,
            score: v[1],
            x0: v[2],
            y0: v[3],
            x1: v[4],
            y1: v[5],
        })
        .collect())
}
