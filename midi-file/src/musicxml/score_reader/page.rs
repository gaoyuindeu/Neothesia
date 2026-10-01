//! Pages of a printed score as black and white bitmaps

use std::path::Path;

/// Pages are rendered at this resolution (scans keep theirs)
const DPI: f32 = 300.0;

/// A gray page, 0 black .. 255 white
pub struct Gray {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

/// A black and white page, true = ink
#[derive(Clone)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub px: Vec<bool>,
}

impl Bitmap {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.px[y * self.w + x]
    }

    /// Ink at a point outside the page counts as paper
    pub fn at(&self, x: i64, y: i64) -> bool {
        x >= 0
            && y >= 0
            && (x as usize) < self.w
            && (y as usize) < self.h
            && self.get(x as usize, y as usize)
    }
}

/// The pages of a PDF (rendered at 300 dpi) or of an image file
pub fn load(path: &Path) -> Result<Vec<Gray>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.starts_with(b"%PDF") {
        return load_pdf(bytes);
    }
    let image = image::load_from_memory(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let gray = image.to_luma8();
    Ok(vec![Gray {
        w: gray.width() as usize,
        h: gray.height() as usize,
        px: gray.into_raw(),
    }])
}

fn load_pdf(bytes: Vec<u8>) -> Result<Vec<Gray>, String> {
    use hayro::hayro_interpret::InterpreterSettings;
    use hayro::hayro_syntax::Pdf;
    use hayro::vello_cpu::color::palette::css::WHITE;
    use hayro::{RenderCache, RenderSettings, render};

    let pdf = Pdf::new(bytes).map_err(|e| format!("Could not read the PDF: {e:?}"))?;
    let cache = RenderCache::new();
    let interpreter = InterpreterSettings::default();
    let scale = DPI / 72.0;
    let settings = RenderSettings {
        x_scale: scale,
        y_scale: scale,
        bg_color: WHITE,
        ..Default::default()
    };
    let mut pages = Vec::new();
    for page in pdf.pages().iter() {
        let pixmap = render(page, &cache, &interpreter, &settings);
        let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
        let rgba = pixmap.data_as_u8_slice();
        let px = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8)
            .collect();
        pages.push(Gray { w, h, px });
    }
    Ok(pages)
}

/// Otsu's threshold of a gray page
fn otsu(gray: &Gray) -> u8 {
    let mut hist = [0u64; 256];
    for &p in &gray.px {
        hist[p as usize] += 1;
    }
    let total = gray.px.len() as f64;
    let sum: f64 = hist
        .iter()
        .enumerate()
        .map(|(i, &n)| i as f64 * n as f64)
        .sum();
    let (mut sum_b, mut w_b, mut best, mut threshold) = (0.0, 0.0, 0.0, 128u8);
    for (t, &n) in hist.iter().enumerate() {
        w_b += n as f64;
        if w_b == 0.0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0.0 {
            break;
        }
        sum_b += t as f64 * n as f64;
        let (m_b, m_f) = (sum_b / w_b, (sum - sum_b) / w_f);
        let between = w_b * w_f * (m_b - m_f) * (m_b - m_f);
        if between > best {
            best = between;
            threshold = t as u8;
        }
    }
    threshold
}

/// Ink where a pixel is darker than both the page threshold (Otsu) and, by a margin, its
/// neighbourhood: uneven lighting of scans does not turn paper black
pub fn binarize(gray: &Gray) -> Bitmap {
    let (w, h) = (gray.w, gray.h);
    let global = otsu(gray) as i64;
    // Integral image for local means
    let mut integral = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            row += gray.px[y * w + x] as u64;
            integral[(y + 1) * (w + 1) + x + 1] = integral[y * (w + 1) + x + 1] + row;
        }
    }
    let r = 25usize;
    let mut px = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let s = integral[y1 * (w + 1) + x1] + integral[y0 * (w + 1) + x0]
                - integral[y0 * (w + 1) + x1]
                - integral[y1 * (w + 1) + x0];
            let mean = s as f64 / ((y1 - y0) * (x1 - x0)) as f64;
            let v = gray.px[y * w + x] as f64;
            px[y * w + x] = (v as i64) <= global + 20 && v < mean * 0.88;
        }
    }
    Bitmap { w, h, px }
}

/// Small rotation of the page (degrees) that makes staff lines horizontal: the angle whose
/// row profile is the most peaked
pub fn skew(page: &Bitmap) -> f32 {
    let step = 4usize;
    let points: Vec<(f32, f32)> = (0..page.h)
        .step_by(step)
        .flat_map(|y| (0..page.w).step_by(step).map(move |x| (x, y)))
        .filter(|&(x, y)| page.get(x, y))
        .map(|(x, y)| (x as f32, y as f32))
        .collect();
    let bins = page.h / step + 1;
    let score = |angle: f32| {
        let t = angle.to_radians().tan();
        let mut hist = vec![0u32; bins + 64];
        for &(x, y) in &points {
            let r = ((y - x * t) / step as f32) as i64 + 32;
            if r >= 0 && (r as usize) < hist.len() {
                hist[r as usize] += 1;
            }
        }
        hist.iter().map(|&n| (n as f64) * (n as f64)).sum::<f64>()
    };
    let mut best = (0.0f32, score(0.0));
    let mut a = -2.0f32;
    while a <= 2.0 {
        let s = score(a);
        if s > best.1 {
            best = (a, s);
        }
        a += 0.1;
    }
    best.0
}

/// The page turned by `-angle` degrees (nearest pixel)
pub fn rotate(page: &Bitmap, angle: f32) -> Bitmap {
    if angle.abs() < 0.05 {
        return page.clone();
    }
    let (s, c) = angle.to_radians().sin_cos();
    let (cx, cy) = (page.w as f32 / 2.0, page.h as f32 / 2.0);
    let mut px = vec![false; page.w * page.h];
    for y in 0..page.h {
        for x in 0..page.w {
            // Where this output pixel comes from
            let (dx, dy) = (x as f32 - cx, y as f32 - cy);
            let sx = c * dx - s * dy + cx;
            let sy = s * dx + c * dy + cy;
            px[y * page.w + x] = page.at(sx.round() as i64, sy.round() as i64);
        }
    }
    Bitmap {
        w: page.w,
        h: page.h,
        px,
    }
}
