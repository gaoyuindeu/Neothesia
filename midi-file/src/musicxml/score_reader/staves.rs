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
    // Line candidates: rows with enough long ink that are the strongest around them (a beam
    // lying on a staff line makes a thick band; its line is the strongest rows of the band,
    // the rest of the band another candidate)
    // (a short staff, a system of one measure, has little long ink)
    let limit = ((peak as f32 * 0.12) as usize).min((6.0 * interline) as usize);
    let reach = thickness as usize + 1;
    let strong = |y: usize| {
        let lo = y.saturating_sub(reach);
        let hi = (y + reach).min(page.h - 1);
        let max = rows[lo..=hi].iter().copied().max().unwrap_or(0);
        rows[y] >= limit && rows[y] as f32 >= 0.8 * max as f32
    };
    let mut lines: Vec<(f32, usize)> = Vec::new(); // (center row, ink)
    let mut y = 0;
    while y < page.h {
        if strong(y) {
            let start = y;
            let mut ink = 0;
            let mut longest = 0;
            let mut weighted = 0.0;
            while y < page.h && strong(y) && y - start <= (3.0 * thickness) as usize + 2 {
                ink += rows[y];
                longest = longest.max(rows[y]);
                weighted += y as f32 * rows[y] as f32;
                y += 1;
            }
            // Strength: the longest row (a staff line runs the width of the staff; ledger
            // lines, often thicker, are short)
            lines.push((weighted / ink as f32, longest));
        } else {
            y += 1;
        }
    }

    // Five lines evenly spaced at about the interline. Other long strokes (beams inside the
    // staff, ledger lines, hairpins) can sit between them: from each candidate as the top
    // line, the candidates nearest to where the next lines should be
    let tolerance = (0.2 * interline).max(2.0);
    // (rows of the five lines, ink)
    let mut options: Vec<([f32; 5], f32)> = Vec::new();
    for top in 0..lines.len() {
        let mut ys = [lines[top].0; 5];
        let mut inks = [lines[top].1; 5];
        let mut last = top;
        let mut real = 1;
        let mut gap = interline;
        let mut ok = true;
        for k in 1..5 {
            let want = ys[k - 1] + gap;
            let next = (last + 1..lines.len())
                .take_while(|&c| lines[c].0 <= want + tolerance)
                .filter(|&c| (lines[c].0 - want).abs() <= tolerance)
                .min_by(|&a, &b| {
                    (lines[a].0 - want)
                        .abs()
                        .total_cmp(&(lines[b].0 - want).abs())
                });
            match next {
                Some(c) => {
                    ys[k] = lines[c].0;
                    inks[k] = lines[c].1;
                    last = c;
                    real += 1;
                }
                // A line inside a band of ink (a beam lying on it): long ink where it should
                // be is enough, from the third line on
                None if k >= 2 => {
                    let lo = (want - tolerance).max(0.0) as usize;
                    let hi = ((want + tolerance) as usize).min(rows.len() - 1);
                    let Some(y) = (lo..=hi).max_by_key(|&y| rows[y]) else {
                        ok = false;
                        break;
                    };
                    if rows[y] < limit {
                        ok = false;
                        break;
                    }
                    ys[k] = y as f32;
                    inks[k] = rows[y];
                    while last + 1 < lines.len() && lines[last + 1].0 <= want + tolerance {
                        last += 1;
                    }
                }
                None => {
                    ok = false;
                    break;
                }
            }
            if k == 1 {
                gap = ys[1] - ys[0];
                if !(0.75 * interline..=1.3 * interline).contains(&gap) {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || real < 3 {
            continue;
        }
        // Staff lines are about equally long; a beam taken for a line is shorter
        let (lo, hi) = (*inks.iter().min().unwrap(), *inks.iter().max().unwrap());
        if (lo as f32) < 0.15 * hi as f32 {
            continue;
        }
        options.push((ys, inks.iter().sum::<usize>() as f32));
    }
    // Strongest staves first, no two overlapping; a weak one (short lines) not right next to
    // a strong one (ledger lines over or under a staff)
    options.sort_by(|a, b| b.1.total_cmp(&a.1));
    let strong = options.first().map_or(0.0, |o| o.1) * 0.3;
    let mut taken: Vec<([f32; 5], f32)> = Vec::new();
    for (ys, ink) in options {
        let clash = taken.iter().any(|(t, _)| {
            // (sharing a line is overlapping too)
            let near = if ink < strong { 2.0 } else { 0.3 } * interline;
            ys[0] <= t[4] + near && ys[4] >= t[0] - near
        });
        if !clash {
            taken.push((ys, ink));
        }
    }
    taken.sort_by(|a, b| a.0[0].total_cmp(&b.0[0]));
    let found: Vec<(Staff, bool)> = taken
        .into_iter()
        .map(|(ys, ink)| {
            let (x0, x1) = extent(page, ys[2].round() as usize, interline);
            (Staff { lines: ys, x0, x1 }, ink >= strong)
        })
        .collect();
    // A weak staff starts at the left margin like the others (a short last system does;
    // ledger lines over a run of notes inside the page do not)
    let mut lefts: Vec<usize> = found.iter().filter(|f| f.1).map(|f| f.0.x0).collect();
    lefts.sort_unstable();
    let margin = lefts.get(lefts.len() / 2).copied().unwrap_or(0) as f32;
    found
        .into_iter()
        .filter(|(st, is_strong)| *is_strong || (st.x0 as f32 - margin).abs() < 3.0 * interline)
        .map(|(st, _)| st)
        .collect()
}

/// Whether two staves (`a` above `b`) are joined by a vertical line at their left end: the
/// barline or bracket that starts a system
pub fn joined(page: &Bitmap, a: &Staff, b: &Staff) -> bool {
    let (y0, y1) = (a.lines[4].round() as usize, b.lines[0].round() as usize);
    if y1 <= y0 + 1 || y1 >= page.h {
        return false;
    }
    // Near the left end of either staff (an end can be off: a box or a label touching the
    // lines lengthens it)
    let reach = (a.spacing() * 4.0) as usize;
    let near = |x0: usize| x0.saturating_sub(reach)..=(x0 + reach).min(page.w - 1);
    near(a.x0).chain(near(b.x0)).any(|x| {
        let ink = (y0..=y1)
            .filter(|&y| (x.saturating_sub(1)..=(x + 1).min(page.w - 1)).any(|xx| page.get(xx, y)))
            .count();
        ink as f32 >= 0.95 * (y1 - y0 + 1) as f32
    })
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

/// Staves grouped into systems of `per_system` staves (piano: 2): in order when they make
/// full systems, else by the line joining the staves of a system at their left end, else by
/// the gaps between them (staves of a system are closer to each other than to the next one)
pub fn systems(page: &Bitmap, staves: &[Staff], per_system: usize) -> Vec<Vec<usize>> {
    if per_system <= 1 || staves.len() < per_system {
        return (0..staves.len()).map(|i| vec![i]).collect();
    }
    let mut joined_groups: Vec<Vec<usize>> = vec![vec![0]];
    for i in 1..staves.len() {
        if joined(page, &staves[i - 1], &staves[i]) {
            joined_groups.last_mut().unwrap().push(i);
        } else {
            joined_groups.push(vec![i]);
        }
    }
    if joined_groups.iter().all(|g| g.len() == per_system) {
        return joined_groups;
    }
    if staves.len().is_multiple_of(per_system) {
        // The usual case: the only grouping into full systems of consecutive staves (gaps
        // can mislead: a system spread for notes between its staves)
        return (0..staves.len() / per_system)
            .map(|s| (s * per_system..(s + 1) * per_system).collect())
            .collect();
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
