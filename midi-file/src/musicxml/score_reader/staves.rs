//! Staff lines: their spacing, the staves of a page and their systems

use super::page::Bitmap;

/// A five line staff
#[derive(Debug, Clone)]
pub struct Staff {
    /// Rows of the lines, top to bottom
    pub lines: [f32; 5],
    pub x0: usize,
    pub x1: usize,
}

impl Staff {
    pub fn spacing(&self) -> f32 {
        (self.lines[4] - self.lines[0]) / 4.0
    }

    /// Staff position of a row: 0 bottom line, 1 the space above, 2 the next line, ...
    pub fn position(&self, y: f32) -> f32 {
        (self.lines[4] - y) / (self.spacing() / 2.0)
    }
}

/// Distance between staff lines and line thickness, from vertical runs: the most common
/// black run is a line, the most common black + white pair a line and a space
pub fn spacing(page: &Bitmap) -> Option<(f32, f32)> {
    let mut black = vec![0u32; 64];
    let mut pair = vec![0u32; 256];
    for x in (0..page.w).step_by(3) {
        let mut y = 0;
        let mut last_black: Option<usize> = None;
        while y < page.h {
            let ink = page.get(x, y);
            let start = y;
            while y < page.h && page.get(x, y) == ink {
                y += 1;
            }
            let len = y - start;
            if ink {
                if len < black.len() {
                    black[len] += 1;
                }
                last_black = Some(len);
            } else if let Some(b) = last_black.take()
                && y < page.h
                && b + len < pair.len()
            {
                pair[b + len] += 1;
            }
        }
    }
    let thickness = (1..black.len()).max_by_key(|&i| black[i])? as f32;
    let interline = (4..pair.len()).max_by_key(|&i| pair[i])? as f32;
    (interline > 2.0 * thickness).then_some((interline, thickness))
}

/// The staves of a deskewed page, top to bottom
pub fn find(page: &Bitmap, interline: f32, thickness: f32) -> Vec<Staff> {
    let long = (2.0 * interline) as usize;
    // Ink of each row in long horizontal runs (lines, not notes or text)
    let mut rows = vec![0usize; page.h];
    for (y, row) in rows.iter_mut().enumerate() {
        let mut x = 0;
        while x < page.w {
            if page.get(x, y) {
                let start = x;
                while x < page.w && page.get(x, y) {
                    x += 1;
                }
                if x - start >= long {
                    *row += x - start;
                }
            } else {
                x += 1;
            }
        }
    }
    let peak = rows.iter().copied().max().unwrap_or(0);
    if peak == 0 {
        return Vec::new();
    }
    // Line candidates: runs of rows with enough long ink
    let limit = (peak as f32 * 0.3) as usize;
    let mut lines: Vec<(f32, usize)> = Vec::new(); // (center row, ink)
    let mut y = 0;
    while y < page.h {
        if rows[y] >= limit {
            let start = y;
            let mut ink = 0;
            let mut weighted = 0.0;
            while y < page.h && rows[y] >= limit && y - start <= (3.0 * thickness) as usize + 2 {
                ink += rows[y];
                weighted += y as f32 * rows[y] as f32;
                y += 1;
            }
            lines.push((weighted / ink as f32, ink));
        } else {
            y += 1;
        }
    }

    // Five lines evenly spaced at about the interline
    let mut staves = Vec::new();
    let mut i = 0;
    while i + 5 <= lines.len() {
        let ys: Vec<f32> = lines[i..i + 5].iter().map(|l| l.0).collect();
        let gaps: Vec<f32> = ys.windows(2).map(|w| w[1] - w[0]).collect();
        let ok = gaps
            .iter()
            .all(|&g| (0.75 * interline..=1.3 * interline).contains(&g));
        if ok {
            let row = ys[2].round() as usize;
            let (x0, x1) = extent(page, row, interline);
            staves.push(Staff {
                lines: [ys[0], ys[1], ys[2], ys[3], ys[4]],
                x0,
                x1,
            });
            i += 5;
        } else {
            i += 1;
        }
    }
    staves
}

/// Left and right end of the line through `row` (gaps shorter than two spaces bridged)
fn extent(page: &Bitmap, row: usize, interline: f32) -> (usize, usize) {
    let near =
        |x: usize| (row.saturating_sub(1)..=(row + 1).min(page.h - 1)).any(|y| page.get(x, y));
    let gap = (2.0 * interline) as usize;
    let mut best = (0, 0);
    let mut x = 0;
    while x < page.w {
        if near(x) {
            let start = x;
            let mut end = x;
            let mut miss = 0;
            while x < page.w && miss <= gap {
                if near(x) {
                    end = x;
                    miss = 0;
                } else {
                    miss += 1;
                }
                x += 1;
            }
            if end - start > best.1 - best.0 {
                best = (start, end);
            }
        } else {
            x += 1;
        }
    }
    best
}

/// Staves grouped into systems of `per_system` staves (piano: 2), by the gaps between them:
/// staves of a system are closer to each other than to the next system
pub fn systems(staves: &[Staff], per_system: usize) -> Vec<Vec<usize>> {
    if per_system <= 1 || staves.len() < per_system {
        return (0..staves.len()).map(|i| vec![i]).collect();
    }
    if staves.len().is_multiple_of(per_system) {
        // The usual case; check that the grouping puts the big gaps between systems
        let gaps: Vec<f32> = staves
            .windows(2)
            .map(|w| w[1].lines[0] - w[0].lines[4])
            .collect();
        let inner: f32 = (0..gaps.len())
            .filter(|i| (i + 1) % per_system != 0)
            .map(|i| gaps[i])
            .fold(0.0, f32::max);
        let outer: f32 = (0..gaps.len())
            .filter(|i| (i + 1) % per_system == 0)
            .map(|i| gaps[i])
            .fold(f32::MAX, f32::min);
        if gaps.len() < per_system || inner <= outer * 1.05 {
            return (0..staves.len() / per_system)
                .map(|s| (s * per_system..(s + 1) * per_system).collect())
                .collect();
        }
    }
    // Otherwise split at the largest gaps
    let gaps: Vec<(usize, f32)> = staves
        .windows(2)
        .enumerate()
        .map(|(i, w)| (i, w[1].lines[0] - w[0].lines[4]))
        .collect();
    let cuts = (staves.len() / per_system).saturating_sub(1);
    let mut largest = gaps.clone();
    largest.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut cut_at: Vec<usize> = largest.iter().take(cuts).map(|g| g.0).collect();
    cut_at.sort();
    let mut out = Vec::new();
    let mut start = 0;
    for c in cut_at {
        out.push((start..=c).collect());
        start = c + 1;
    }
    out.push((start..staves.len()).collect());
    out
}
