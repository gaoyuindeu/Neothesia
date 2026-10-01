//! Reading fingering digits on pages recognized by Audiveris.
//!
//! Audiveris finds the staves, notes and chords of a page well, but its digit classifier
//! misses many fingering numbers (italic 3s become tuplet marks, others are dropped). This
//! goes over its project file (.omr) again:
//!
//! 1. Everything Audiveris recognized with confidence (heads, stems, beams, clefs, ...) and
//!    the staff lines are erased from the binarized page.
//! 2. What is left is split into connected components; pieces a staff line cut apart are
//!    joined again; digit-sized ones are compared with templates of 1-5 in several free
//!    fonts (upright and slanted). Digits found with confidence become templates of the
//!    score's own font for a second pass.
//! 3. Numbers written side by side (tempo marks, measure numbers) and letters of words are
//!    left out. Digits stacked on top of each other finger a chord: top digit, top note.
//!    Each stack goes to the chord it is best aligned with; a row of single digits at one
//!    height goes to one staff.
//! 4. The project gets these digits as fingering attached to their heads, replacing what
//!    Audiveris guessed, so that its MusicXML export carries them.

use std::{collections::HashMap, io::Read, sync::OnceLock};

use roxmltree::{Document, Node};

const TW: usize = 20;
const TH: usize = 28;

/// Inters that are certainly not fingering: their pixels are erased
const STRUCTURAL: &[&str] = &[
    "head",
    "stem",
    "beam",
    "beam-hook",
    "ledger",
    "flag",
    "barline",
    "bar-connector",
    "clef",
    "key-alter",
    "time-whole",
    "time-number",
    "time-pair",
    "alter",
    "rest",
    "augmentation-dot",
    "brace",
    "bracket",
    "slur",
    "articulation",
    "ornament",
    "arpeggiato",
    "wedge",
    "small-flag",
    "small-beam",
    "grace-chord",
];
/// Small solid symbols: erased with their box and a margin (anti-aliased edges)
const COMPACT: &[&str] = &[
    "head",
    "stem",
    "ledger",
    "augmentation-dot",
    "alter",
    "flag",
    "rest",
    "articulation",
    "key-alter",
    "barline",
    "bar-connector",
    "clef",
    "time-whole",
    "time-number",
    "brace",
];
/// Audiveris' own guesses that may be fingering: replaced by ours
const GUESSES: &[&str] = &["fingering", "tuplet", "dynamics"];

/// What the reading found
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DigitReport {
    pub digits: usize,
    pub attached: usize,
}

struct Template {
    digit: u8,
    aspect: f32,
    v: Vec<f32>,
}

fn unit(mut v: Vec<f32>) -> Option<Vec<f32>> {
    let mean = v.iter().sum::<f32>() / v.len() as f32;
    v.iter_mut().for_each(|x| *x -= mean);
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (norm > 0.0).then(|| v.into_iter().map(|x| x / norm).collect())
}

/// Templates of the digits in free fonts (made by tools/make_digit_templates.py)
fn templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let data: &[u8] = include_bytes!("digit_templates.bin");
        let count = u16::from_le_bytes([data[4], data[5]]) as usize;
        let (w, h) = (data[6] as usize, data[7] as usize);
        assert_eq!((w, h), (TW, TH));
        let mut out = Vec::with_capacity(count);
        let mut at = 8;
        for _ in 0..count {
            let digit = data[at];
            let aspect = f32::from_le_bytes(data[at + 1..at + 5].try_into().unwrap());
            let pixels = &data[at + 5..at + 5 + w * h];
            at += 5 + w * h;
            if let Some(v) = unit(pixels.iter().map(|&p| p as f32 / 255.0).collect()) {
                out.push(Template { digit, aspect, v });
            }
        }
        out
    })
}

/// A black and white image
#[derive(Clone)]
struct Bitmap {
    w: usize,
    h: usize,
    px: Vec<bool>,
}

impl Bitmap {
    fn get(&self, x: usize, y: usize) -> bool {
        self.px[y * self.w + x]
    }

    fn set(&mut self, x: usize, y: usize, v: bool) {
        self.px[y * self.w + x] = v;
    }

    fn clear_rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64) {
        let (x0, y0) = (x0.max(0) as usize, y0.max(0) as usize);
        let (x1, y1) = (
            (x1.max(0) as usize).min(self.w),
            (y1.max(0) as usize).min(self.h),
        );
        for y in y0..y1 {
            for x in x0..x1 {
                self.set(x, y, false);
            }
        }
    }
}

/// Resampling like PIL's bilinear filter (a tent widened when shrinking)
/// `src` holds `count` lines of `n_in` samples; each line is resampled to `n_out`
fn resample_axis(src: &[f32], n_in: usize, count: usize, n_out: usize) -> Vec<f32> {
    let scale = n_in as f32 / n_out as f32;
    let support = scale.max(1.0);
    let mut weights: Vec<(usize, Vec<f32>)> = Vec::with_capacity(n_out);
    for o in 0..n_out {
        let center = (o as f32 + 0.5) * scale;
        let lo = ((center - support).floor().max(0.0)) as usize;
        let hi = ((center + support).ceil() as usize).min(n_in);
        let mut ws: Vec<f32> = (lo..hi)
            .map(|i| {
                let d = ((i as f32 + 0.5 - center) / support).abs();
                (1.0 - d).max(0.0)
            })
            .collect();
        let total: f32 = ws.iter().sum();
        if total > 0.0 {
            ws.iter_mut().for_each(|w| *w /= total);
        }
        weights.push((lo, ws));
    }
    let mut out = vec![0.0; count * n_out];
    for j in 0..count {
        let line = &src[j * n_in..(j + 1) * n_in];
        for (o, (lo, ws)) in weights.iter().enumerate() {
            out[j * n_out + o] = ws.iter().enumerate().map(|(k, w)| w * line[lo + k]).sum();
        }
    }
    out
}

/// Crop a mask to its ink, scale to the template size, zero mean, unit length
fn normalize(mask: &Bitmap) -> Option<(Vec<f32>, f32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..mask.h {
        for x in 0..mask.w {
            if mask.get(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 == usize::MAX {
        return None;
    }
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut rows = Vec::with_capacity(w * h);
    for y in y0..=y1 {
        for x in x0..=x1 {
            rows.push(if mask.get(x, y) { 1.0 } else { 0.0 });
        }
    }
    // Horizontal pass (h lines of w), then vertical (transpose, TW lines of h)
    let horizontal = resample_axis(&rows, w, h, TW);
    let mut columns = vec![0.0; TW * h];
    for y in 0..h {
        for x in 0..TW {
            columns[x * h + y] = horizontal[y * TW + x];
        }
    }
    let vertical = resample_axis(&columns, h, TW, TH);
    let mut v = vec![0.0; TW * TH];
    for x in 0..TW {
        for y in 0..TH {
            v[y * TW + x] = vertical[x * TH + y];
        }
    }
    unit(v).map(|v| (v, w as f32 / h as f32))
}

/// Best digit, its score and its lead over the second best
/// Bold print (heavy scans): erode until the ink share looks like a normal font
fn thin(mask: &Bitmap) -> Bitmap {
    let mut mask = mask.clone();
    for _ in 0..2 {
        let ink = mask.px.iter().filter(|&&p| p).count();
        if ink == 0 {
            break;
        }
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
        for y in 0..mask.h {
            for x in 0..mask.w {
                if mask.get(x, y) {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        let area = (x1 - x0 + 1) * (y1 - y0 + 1);
        if ink as f32 / area as f32 <= 0.45 {
            break;
        }
        let mut eroded = mask.clone();
        for y in 0..mask.h {
            for x in 0..mask.w {
                let keep = mask.get(x, y)
                    && (y == 0 || mask.get(x, y - 1))
                    && (y + 1 == mask.h || mask.get(x, y + 1))
                    && (x == 0 || mask.get(x - 1, y))
                    && (x + 1 == mask.w || mask.get(x + 1, y));
                eroded.set(x, y, keep);
            }
        }
        if (eroded.px.iter().filter(|&&p| p).count() as f32) < 0.4 * ink as f32 {
            break;
        }
        mask = eroded;
    }
    mask
}

fn classify(mask: &Bitmap, own: &[Template]) -> Option<(u8, f32, f32, Vec<f32>, f32)> {
    let (v, aspect) = normalize(&thin(mask))?;
    let mut best = [f32::MIN; 6];
    for t in templates().iter().chain(own) {
        let dot: f32 = v.iter().zip(&t.v).map(|(a, b)| a * b).sum();
        let score = dot - 0.5 * ((aspect + 0.05) / (t.aspect + 0.05)).ln().abs();
        let d = t.digit as usize;
        best[d] = best[d].max(score);
    }
    let mut ranked: Vec<(u8, f32)> = (1..=5u8).map(|d| (d, best[d as usize])).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1));
    Some((
        ranked[0].0,
        ranked[0].1,
        ranked[0].1 - ranked[1].1,
        v,
        aspect,
    ))
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Rect {
    fn cx(&self) -> f32 {
        (self.x0 + self.x1) as f32 / 2.0
    }

    fn cy(&self) -> f32 {
        (self.y0 + self.y1) as f32 / 2.0
    }
}

fn bounds(node: Node) -> Option<Rect> {
    let b = node.children().find(|c| c.has_tag_name("bounds"))?;
    let get = |k: &str| -> Option<i64> { b.attribute(k)?.parse::<f64>().ok().map(|v| v as i64) };
    let (x, y, w, h) = (get("x")?, get("y")?, get("w")?, get("h")?);
    Some(Rect {
        x0: x,
        y0: y,
        x1: x + w,
        y1: y + h,
    })
}

struct Head {
    id: String,
    staff: String,
    rect: Rect,
    system: usize,
}

struct Digit {
    d: u8,
    rect: Rect,
}

/// Everything read from one sheet of the project
struct SheetData {
    interline: f32,
    page: Bitmap,
    heads: Vec<Head>,
    chords: Vec<(Rect, String)>,
    /// Staff lines: thickness and points
    lines: Vec<(f32, Vec<(f32, f32)>)>,
}

fn glyph_masks(doc: &Document) -> HashMap<String, (i64, i64, Bitmap)> {
    let mut out = HashMap::new();
    for g in doc.descendants().filter(|n| n.has_tag_name("glyph")) {
        let (Some(id), Some(rt)) = (
            g.attribute("id"),
            g.children().find(|c| c.has_tag_name("run-table")),
        ) else {
            continue;
        };
        let num = |n: Node, k: &str| n.attribute(k).and_then(|v| v.parse::<i64>().ok());
        let (Some(w), Some(h), Some(left), Some(top)) = (
            num(rt, "width"),
            num(rt, "height"),
            num(g, "left"),
            num(g, "top"),
        ) else {
            continue;
        };
        let vertical = rt.attribute("orientation") == Some("VERTICAL");
        let mut mask = Bitmap {
            w: w as usize,
            h: h as usize,
            px: vec![false; (w * h) as usize],
        };
        for (i, runs) in rt.children().filter(|c| c.has_tag_name("runs")).enumerate() {
            let mut pos = 0usize;
            let mut fg = true;
            for n in runs.text().unwrap_or("").split_whitespace() {
                let n: usize = n.parse().unwrap_or(0);
                if fg {
                    for k in pos..pos + n {
                        if vertical && k < mask.h && i < mask.w {
                            mask.set(i, k, true);
                        } else if !vertical && k < mask.w && i < mask.h {
                            mask.set(k, i, true);
                        }
                    }
                }
                pos += n;
                fg = !fg;
            }
        }
        out.insert(id.to_string(), (left, top, mask));
    }
    out
}

fn read_sheet(doc: &Document, binary: Bitmap) -> SheetData {
    let root = doc.root_element();
    let interline = root
        .descendants()
        .find(|n| n.has_tag_name("interline"))
        .and_then(|n| n.attribute("main"))
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(20.0);
    let masks = glyph_masks(doc);
    let mut page = binary;
    let mut heads = Vec::new();
    let mut chords = Vec::new();
    let mut lines = Vec::new();

    for (system_index, system) in root
        .descendants()
        .filter(|n| n.has_tag_name("system"))
        .enumerate()
    {
        for line in system
            .descendants()
            .filter(|n| n.has_tag_name("lines"))
            .flat_map(|l| l.children().filter(|c| c.has_tag_name("line")))
        {
            let thickness = line
                .attribute("thickness")
                .and_then(|t| t.parse::<f32>().ok())
                .unwrap_or(2.0);
            let points: Vec<(f32, f32)> = line
                .children()
                .filter(|p| p.has_tag_name("point"))
                .filter_map(|p| {
                    Some((
                        p.attribute("x")?.parse::<f32>().ok()?.trunc(),
                        p.attribute("y")?.parse::<f32>().ok()?,
                    ))
                })
                .collect();
            if points.len() >= 2 {
                lines.push((thickness, points));
            }
        }

        let Some(inters) = system
            .children()
            .find(|c| c.has_tag_name("sig"))
            .and_then(|sig| sig.children().find(|c| c.has_tag_name("inters")))
        else {
            continue;
        };
        for e in inters.children().filter(|c| c.is_element()) {
            let tag = e.tag_name().name();
            let grade: f32 = e
                .attribute("grade")
                .and_then(|g| g.parse().ok())
                .unwrap_or(0.0);
            if STRUCTURAL.contains(&tag) && grade >= 0.35 {
                if let (true, Some(b)) = (COMPACT.contains(&tag), bounds(e)) {
                    page.clear_rect(b.x0 - 2, b.y0 - 2, b.x1 + 2, b.y1 + 2);
                } else if let Some((left, top, m)) = e.attribute("glyph").and_then(|g| masks.get(g))
                {
                    for y in 0..m.h as i64 {
                        for x in 0..m.w as i64 {
                            let near = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(
                                |(dx, dy)| {
                                    let (nx, ny) = (x + dx, y + dy);
                                    nx >= 0
                                        && ny >= 0
                                        && (nx as usize) < m.w
                                        && (ny as usize) < m.h
                                        && m.get(nx as usize, ny as usize)
                                },
                            );
                            let (px, py) = (left + x, top + y);
                            if near
                                && px >= 0
                                && py >= 0
                                && (px as usize) < page.w
                                && (py as usize) < page.h
                            {
                                page.set(px as usize, py as usize, false);
                            }
                        }
                    }
                }
            }
            match tag {
                "head" => {
                    if let Some(rect) = bounds(e) {
                        heads.push(Head {
                            id: e.attribute("id").unwrap_or_default().to_string(),
                            staff: e.attribute("staff").unwrap_or_default().to_string(),
                            rect,
                            system: system_index,
                        });
                    }
                }
                "head-chord" => {
                    if let Some(rect) = bounds(e) {
                        chords.push((rect, e.attribute("staff").unwrap_or_default().to_string()));
                    }
                }
                _ => {}
            }
        }
    }

    // Staff lines: clear the line where nothing crosses it
    for (thickness, points) in &lines {
        let half = (thickness / 2.0 + 1.5) as i64;
        for pair in points.windows(2) {
            let ((xa, ya), (xb, yb)) = (pair[0], pair[1]);
            let (xa, xb) = (xa as i64, xb as i64);
            for x in xa.max(0)..=xb.min(page.w as i64 - 1) {
                let yl = ya + (yb - ya) * (x - xa) as f32 / (xb - xa).max(1) as f32;
                let (r0, r1) = (yl.round() as i64 - half, yl.round() as i64 + half);
                if r0 < 1 || r1 >= page.h as i64 - 1 {
                    continue;
                }
                let x = x as usize;
                let crossed = page.get(x, (r0 - 1) as usize) && page.get(x, (r1 + 1) as usize);
                if !crossed {
                    for y in r0..=r1 {
                        page.set(x, y as usize, false);
                    }
                }
            }
        }
    }

    SheetData {
        interline,
        page,
        heads,
        chords,
        lines,
    }
}

/// A connected component: bounding box and pixels
struct Component {
    rect: Rect,
    pixels: Vec<(u32, u32)>,
}

fn components(page: &Bitmap) -> Vec<Component> {
    let mut seen = vec![false; page.px.len()];
    let mut out = Vec::new();
    let mut stack = Vec::new();
    for start in 0..page.px.len() {
        if !page.px[start] || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut pixels = Vec::new();
        let mut rect = Rect {
            x0: i64::MAX,
            y0: i64::MAX,
            x1: 0,
            y1: 0,
        };
        while let Some(i) = stack.pop() {
            let (x, y) = (i % page.w, i / page.w);
            pixels.push((x as u32, y as u32));
            rect.x0 = rect.x0.min(x as i64);
            rect.y0 = rect.y0.min(y as i64);
            rect.x1 = rect.x1.max(x as i64);
            rect.y1 = rect.y1.max(y as i64);
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                    if nx < 0 || ny < 0 || nx >= page.w as i64 || ny >= page.h as i64 {
                        continue;
                    }
                    let j = ny as usize * page.w + nx as usize;
                    if page.px[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        out.push(Component { rect, pixels });
    }
    out
}

/// Rows of the staff lines at `x`
fn line_rows(lines: &[(f32, Vec<(f32, f32)>)], x: f32) -> Vec<f32> {
    lines
        .iter()
        .filter_map(|(_, pts)| {
            let ((xa, ya), (xb, yb)) = (pts[0], pts[pts.len() - 1]);
            (xa <= x && x <= xb).then(|| ya + (yb - ya) * (x - xa) / (xb - xa).max(1.0))
        })
        .collect()
}

/// Components of the cleaned page, with pieces a staff line cut apart joined again
fn candidate_components(sheet: &SheetData) -> Vec<Component> {
    let il = sheet.interline;
    let mut comps: Vec<Component> = components(&sheet.page)
        .into_iter()
        .filter(|c| c.rect.y1 - c.rect.y0 + 1 >= 3 || c.rect.x1 - c.rect.x0 + 1 >= 3)
        .collect();
    loop {
        comps.sort_by_key(|c| (c.rect.x0, c.rect.y0));
        let mut pair = None;
        'outer: for i in 0..comps.len() {
            for j in i + 1..comps.len() {
                let (a, b) = (comps[i].rect, comps[j].rect);
                if b.x0 > a.x1 + 2 {
                    break;
                }
                let overlap = a.x1.min(b.x1) - a.x0.max(b.x0);
                let gap = a.y0.max(b.y0) - a.y1.min(b.y1);
                let height = a.y1.max(b.y1) - a.y0.min(b.y0);
                let mid = (a.y1.min(b.y1) + a.y0.max(b.y0)) as f32 / 2.0;
                let at_line = line_rows(&sheet.lines, a.x0 as f32)
                    .iter()
                    .any(|y| (mid - y).abs() <= 3.0);
                if overlap >= 1 && gap <= 8 && at_line && height as f32 <= 1.8 * il {
                    pair = Some((i, j));
                    break 'outer;
                }
            }
        }
        let Some((i, j)) = pair else {
            break;
        };
        let b = comps.remove(j);
        let a = &mut comps[i];
        a.rect = Rect {
            x0: a.rect.x0.min(b.rect.x0),
            y0: a.rect.y0.min(b.rect.y0),
            x1: a.rect.x1.max(b.rect.x1),
            y1: a.rect.y1.max(b.rect.y1),
        };
        a.pixels.extend(b.pixels);
    }
    comps.into_iter().flat_map(|c| split_stack(c, il)).collect()
}

/// Digits of a chord stacked so close that they touch: cut at the thin rows
fn split_stack(c: Component, il: f32) -> Vec<Component> {
    let (w, h) = (
        (c.rect.x1 - c.rect.x0 + 1) as usize,
        (c.rect.y1 - c.rect.y0 + 1) as usize,
    );
    if !(h as f32 > 1.8 * il && h as f32 <= 6.0 * il && (0.2 * il..=1.4 * il).contains(&(w as f32)))
    {
        return vec![c];
    }
    let mut profile = vec![0usize; h];
    for &(_, y) in &c.pixels {
        profile[y as usize - c.rect.y0 as usize] += 1;
    }
    let limit = ((0.2 * w as f32) as usize).max(1);
    let neck: Vec<bool> = profile.iter().map(|&n| n <= limit).collect();
    let mut cuts = vec![0usize];
    let mut y = 0;
    while y < h {
        if neck[y] {
            let start = y;
            while y < h && neck[y] {
                y += 1;
            }
            let cut = (start + y) / 2;
            if (cut - cuts[cuts.len() - 1]) as f32 >= 0.45 * il && (h - cut) as f32 >= 0.45 * il {
                cuts.push(cut);
            }
        }
        y += 1;
    }
    cuts.push(h);
    if cuts.len() <= 2 {
        return vec![c];
    }
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (top, bottom) = (pair[0], pair[1]);
        let pixels: Vec<(u32, u32)> = c
            .pixels
            .iter()
            .copied()
            .filter(|&(_, y)| {
                let r = y as usize - c.rect.y0 as usize;
                r >= top && r < bottom && profile[r] > limit
            })
            .collect();
        if pixels.is_empty() {
            continue;
        }
        let rect = Rect {
            x0: pixels.iter().map(|p| p.0 as i64).min().unwrap(),
            y0: pixels.iter().map(|p| p.1 as i64).min().unwrap(),
            x1: pixels.iter().map(|p| p.0 as i64).max().unwrap(),
            y1: pixels.iter().map(|p| p.1 as i64).max().unwrap(),
        };
        out.push(Component { rect, pixels });
    }
    out
}

fn mask_of(c: &Component) -> Bitmap {
    let (w, h) = (
        (c.rect.x1 - c.rect.x0 + 1) as usize,
        (c.rect.y1 - c.rect.y0 + 1) as usize,
    );
    let mut m = Bitmap {
        w,
        h,
        px: vec![false; w * h],
    };
    for &(x, y) in &c.pixels {
        m.set(
            x as usize - c.rect.x0 as usize,
            y as usize - c.rect.y0 as usize,
            true,
        );
    }
    m
}

fn digit_sized(c: &Component, il: f32) -> bool {
    let (w, h) = (
        (c.rect.x1 - c.rect.x0 + 1) as f32,
        (c.rect.y1 - c.rect.y0 + 1) as f32,
    );
    (0.5 * il..=1.8 * il).contains(&h) && (0.2 * il..=1.4 * il).contains(&w)
}

/// A digit seen clearly enough to learn the score's font from
struct Example {
    margin: f32,
    confident: bool,
    template: Template,
}

/// Digits of a sheet; `own` are the templates of the score's font (second pass)
fn find_digits(
    sheet: &SheetData,
    comps: &[Component],
    own: &[Template],
    examples: &mut Vec<Example>,
) -> Vec<Digit> {
    let il = sheet.interline;
    let threshold = if own.is_empty() { 0.55 } else { 0.62 };
    let mut digits = Vec::new();
    for c in comps.iter().filter(|c| digit_sized(c, il)) {
        let Some((d, score, margin, v, aspect)) = classify(&mask_of(c), own) else {
            continue;
        };
        if score >= 0.45 && margin >= 0.15 {
            examples.push(Example {
                margin,
                confident: score >= 0.7,
                template: Template {
                    digit: d,
                    aspect,
                    v,
                },
            });
        }
        if (score >= threshold && margin >= 0.04) || (score >= 0.5 && margin >= 0.2) {
            digits.push(Digit { d, rect: c.rect });
        }
    }

    // Side by side with anything (a word, a number of several digits): not fingering
    let others: Vec<Rect> = comps
        .iter()
        .map(|c| c.rect)
        .filter(|r| (r.y1 - r.y0 + 1) as f32 >= 0.3 * il)
        .collect();
    let side_by_side = |a: &Rect, b: &Rect| {
        let overlap = a.y1.min(b.y1) - a.y0.max(b.y0);
        let gap = a.x0.max(b.x0) - a.x1.min(b.x1);
        overlap as f32 > 0.5 * (a.y1 - a.y0).min(b.y1 - b.y0) as f32 && (gap as f32) < 0.35 * il
    };
    digits.retain(|d| {
        !others
            .iter()
            .any(|o| (o.x0, o.y0) != (d.rect.x0, d.rect.y0) && side_by_side(&d.rect, o))
    });
    digits
}

/// The score's own templates: the mean of its confident digits, or of the clearest
/// plausible ones when its font is far from the generic templates (bold scans)
fn own_templates(examples: Vec<Example>) -> Vec<Template> {
    let confident_digits = (1..=5u8)
        .filter(|&d| {
            examples
                .iter()
                .filter(|e| e.confident && e.template.digit == d)
                .count()
                >= 5
        })
        .count();
    let mut chosen: Vec<Template> = Vec::new();
    if confident_digits >= 3 {
        chosen.extend(
            examples
                .into_iter()
                .filter(|e| e.confident)
                .map(|e| e.template),
        );
    } else {
        for d in 1..=5u8 {
            let mut found: Vec<Example> = Vec::new();
            for e in examples.iter().filter(|e| e.template.digit == d) {
                found.push(Example {
                    margin: e.margin,
                    confident: e.confident,
                    template: Template {
                        digit: d,
                        aspect: e.template.aspect,
                        v: e.template.v.clone(),
                    },
                });
            }
            found.sort_by(|a, b| b.margin.total_cmp(&a.margin));
            chosen.extend(found.into_iter().take(30).map(|e| e.template));
        }
    }
    let confident = chosen;
    let mut out = Vec::new();
    for d in 1..=5u8 {
        let examples: Vec<&Template> = confident.iter().filter(|t| t.digit == d).collect();
        if examples.len() < 2 {
            continue;
        }
        let mut v = vec![0.0f32; TW * TH];
        for e in &examples {
            v.iter_mut().zip(&e.v).for_each(|(a, b)| *a += b);
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            continue;
        }
        v.iter_mut().for_each(|x| *x /= norm);
        let mut aspects: Vec<f32> = examples.iter().map(|e| e.aspect).collect();
        aspects.sort_by(f32::total_cmp);
        let aspect = if aspects.len() % 2 == 1 {
            aspects[aspects.len() / 2]
        } else {
            (aspects[aspects.len() / 2 - 1] + aspects[aspects.len() / 2]) / 2.0
        };
        out.push(Template {
            digit: d,
            aspect,
            v,
        });
    }
    out
}

/// (digit, head) pairs
fn attach(sheet: &SheetData, digits: Vec<Digit>) -> Vec<(Digit, usize)> {
    let il = sheet.interline;

    // Stacks of digits above one another
    let mut sorted = digits;
    sorted.sort_by_key(|d| d.rect.y0);
    let mut stacks: Vec<Vec<Digit>> = Vec::new();
    for d in sorted {
        let cx = d.rect.cx();
        let home = stacks.iter_mut().find(|st| {
            let last = st.last().unwrap().rect;
            let dy = d.rect.y0 - last.y1;
            (cx - last.cx()).abs() < 0.6 * il && dy >= -2 && (dy as f32) < 0.7 * il
        });
        match home {
            Some(st) => st.push(d),
            None => stacks.push(vec![d]),
        }
    }

    // Heads of each chord, top to bottom
    let chord_heads: Vec<Vec<usize>> = sheet
        .chords
        .iter()
        .filter_map(|(r, staff)| {
            let mut members: Vec<usize> = (0..sheet.heads.len())
                .filter(|&i| {
                    let h = &sheet.heads[i];
                    h.staff == *staff
                        && (r.x0 as f32 - 0.7 * il..=r.x1 as f32 + 0.7 * il).contains(&h.rect.cx())
                        && (r.y0 as f32 - 2.0..=r.y1 as f32 + 2.0).contains(&h.rect.cy())
                })
                .collect();
            members.sort_by_key(|&i| sheet.heads[i].rect.y0);
            (!members.is_empty()).then_some(members)
        })
        .collect();

    // The chords a stack could belong to, best first: (score, chord, above?, staff)
    let candidates = |st: &[Digit]| -> Vec<(f32, usize, bool, String)> {
        let scx = st.iter().map(|d| d.rect.cx()).sum::<f32>() / st.len() as f32;
        let (sy0, sy1) = (st[0].rect.y0, st[st.len() - 1].rect.y1);
        let mut out = Vec::new();
        for (c, heads) in chord_heads.iter().enumerate() {
            let hx0 = heads.iter().map(|&i| sheet.heads[i].rect.x0).min().unwrap() as f32;
            let hx1 = heads.iter().map(|&i| sheet.heads[i].rect.x1).max().unwrap() as f32;
            if !(hx0 - 0.7 * il..=hx1 + 0.7 * il).contains(&scx) {
                continue;
            }
            let top = sheet.heads[heads[0]].rect.y0;
            let bottom = sheet.heads[heads[heads.len() - 1]].rect.y1;
            let (dist, above) = if sy1 <= top + 2 {
                (top - sy1, true)
            } else if sy0 >= bottom - 2 {
                (sy0 - bottom, false)
            } else {
                continue;
            };
            if dist as f32 > 10.0 * il {
                continue;
            }
            let score = 2.0 * (scx - (hx0 + hx1) / 2.0).abs() / il + 0.3 * dist as f32 / il;
            out.push((score, c, above, sheet.heads[heads[0]].staff.clone()));
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    };
    let cands: Vec<_> = stacks.iter().map(|st| candidates(st)).collect();

    // A row of single digits at one height belongs to one staff
    let mut singles: Vec<usize> = (0..stacks.len())
        .filter(|&i| stacks[i].len() == 1 && !cands[i].is_empty())
        .collect();
    singles.sort_by_key(|&i| stacks[i][0].rect.y0);
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in singles {
        let d = stacks[i][0].rect;
        let row = rows.iter_mut().find(|row| {
            let r = stacks[*row.last().unwrap()][0].rect;
            ((d.y0 - r.y0).abs() as f32) < 0.6 * il && ((d.x0 - r.x0).abs() as f32) < 6.0 * il
        });
        match row {
            Some(row) => row.push(i),
            None => rows.push(vec![i]),
        }
    }
    let mut staff_of: HashMap<usize, String> = HashMap::new();
    for row in rows.iter().filter(|r| r.len() >= 3) {
        let mut votes: HashMap<&str, usize> = HashMap::new();
        for &i in row {
            *votes.entry(cands[i][0].3.as_str()).or_default() += 1;
        }
        let staff = votes
            .into_iter()
            .max_by_key(|(staff, n)| (*n, std::cmp::Reverse(staff.to_string())))
            .unwrap()
            .0
            .to_string();
        for &i in row {
            staff_of.insert(i, staff.clone());
        }
    }

    let mut links = Vec::new();
    let mut used = vec![false; sheet.heads.len()];
    for (i, st) in stacks.into_iter().enumerate() {
        let options: Vec<&(f32, usize, bool, String)> = match staff_of.get(&i) {
            Some(staff) => {
                let o: Vec<_> = cands[i].iter().filter(|c| &c.3 == staff).collect();
                if o.is_empty() {
                    cands[i].iter().collect()
                } else {
                    o
                }
            }
            None => cands[i].iter().collect(),
        };
        let Some(&&(_, chord, above, _)) = options.first() else {
            continue;
        };
        let heads = &chord_heads[chord];
        let mut st = st;
        // Top digit for the top note; with fewer digits than notes, from the stack's side
        if st.len() > heads.len() {
            st = if above {
                st.split_off(st.len() - heads.len())
            } else {
                st.truncate(heads.len());
                st
            };
        }
        let targets: Vec<usize> = if above {
            heads[..st.len()].to_vec()
        } else {
            heads[heads.len() - st.len()..].to_vec()
        };
        for (d, h) in st.into_iter().zip(targets) {
            if !used[h] {
                used[h] = true;
                links.push((d, h));
            }
        }
    }
    links
}

fn decode_binary(png_bytes: &[u8]) -> Result<Bitmap, String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| e.to_string())?;
    let (w, h) = (info.width as usize, info.height as usize);
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => 3,
    };
    let stride = info.line_size;
    let mut px = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            px[y * w + x] = buf[y * stride + x * channels] < 128;
        }
    }
    Ok(Bitmap { w, h, px })
}

/// One sheet of the project: its XML and binarized page
struct SheetFile {
    xml_name: String,
    xml: String,
    binary: Bitmap,
}

fn parse(xml: &str) -> Result<Document<'_>, String> {
    Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())
}

/// The sheet XML with Audiveris' fingering guesses replaced by `links`
fn patch_sheet(xml: &str, sheet: &SheetData, links: &[(Digit, usize)]) -> Result<String, String> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    let systems: Vec<Node> = root
        .descendants()
        .filter(|n| n.has_tag_name("system"))
        .collect();

    // (start, end, replacement)
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut dropped = std::collections::HashSet::new();
    for system in &systems {
        let Some(sig) = system.children().find(|c| c.has_tag_name("sig")) else {
            continue;
        };
        if let Some(inters) = sig.children().find(|c| c.has_tag_name("inters")) {
            for e in inters.children().filter(|c| c.is_element()) {
                if GUESSES.contains(&e.tag_name().name()) {
                    dropped.insert(e.attribute("id").unwrap_or_default().to_string());
                    let r = e.range();
                    edits.push((r.start, r.end, String::new()));
                }
            }
        }
    }
    for rel in root.descendants().filter(|n| n.has_tag_name("relation")) {
        let touches = |k: &str| rel.attribute(k).is_some_and(|v| dropped.contains(v));
        if touches("source") || touches("target") {
            let r = rel.range();
            edits.push((r.start, r.end, String::new()));
        }
    }

    let mut next_id: u64 = root
        .attribute("last-persistent-id")
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);
    let mut inters_text: HashMap<usize, String> = HashMap::new();
    let mut relations_text: HashMap<usize, String> = HashMap::new();
    for (d, h) in links {
        let head = &sheet.heads[*h];
        next_id += 1;
        let r = d.rect;
        inters_text.entry(head.system).or_default().push_str(&format!(
            "<fingering value=\"{0}\" shape=\"DIGIT_{0}\" grade=\"0.9\" ctx-grade=\"0.9\" id=\"{1}\">\
             <bounds x=\"{2}\" y=\"{3}\" w=\"{4}\" h=\"{5}\"/></fingering>\n",
            d.d,
            next_id,
            r.x0,
            r.y0,
            r.x1 - r.x0 + 1,
            r.y1 - r.y0 + 1
        ));
        relations_text.entry(head.system).or_default().push_str(&format!(
            "<relation source=\"{}\" target=\"{}\"><head-fingering dx=\"0\" dy=\"0\" grade=\"1\"/></relation>\n",
            head.id, next_id
        ));
    }
    for (i, system) in systems.iter().enumerate() {
        let Some(sig) = system.children().find(|c| c.has_tag_name("sig")) else {
            continue;
        };
        for (tag, texts) in [("inters", &inters_text), ("relations", &relations_text)] {
            let Some(text) = texts.get(&i) else {
                continue;
            };
            let Some(el) = sig.children().find(|c| c.has_tag_name(tag)) else {
                return Err(format!("system without <{tag}>"));
            };
            let r = el.range();
            let close = format!("</{tag}>");
            if !xml[r.clone()].ends_with(&close) {
                // <inters/>: write it out
                edits.push((r.start, r.end, format!("<{tag}>{text}</{tag}>")));
            } else {
                let at = r.end - close.len();
                edits.push((at, at, text.clone()));
            }
        }
    }

    // The id counter in the root tag
    let root_start = root.range().start;
    let tag_end = root_start + xml[root_start..].find('>').ok_or("broken sheet")?;
    if let Some(pos) = xml[root_start..tag_end].find("last-persistent-id=\"") {
        let value_start = root_start + pos + "last-persistent-id=\"".len();
        let value_end = value_start + xml[value_start..].find('"').ok_or("broken sheet")?;
        edits.push((value_start, value_end, next_id.to_string()));
    }

    edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = xml.to_string();
    let mut last_start = usize::MAX;
    for (start, end, text) in edits {
        // Nested removals (a relation inside a removed range) are skipped
        if end > last_start {
            continue;
        }
        out.replace_range(start..end, &text);
        last_start = start;
    }
    Ok(out)
}

/// Read the fingering digits of an Audiveris project (.omr file contents) and return the
/// project with them attached to their notes
pub fn patch_omr(omr: &[u8]) -> Result<(Vec<u8>, DigitReport), String> {
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(omr)).map_err(|e| format!("Broken .omr: {e}"))?;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        entries.push((f.name().to_string(), data));
    }
    let files: HashMap<&str, &[u8]> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();

    let mut sheets = Vec::new();
    for (name, data) in &entries {
        let Some(dir) = name.strip_suffix(".xml").and_then(|n| n.rsplit_once('/')) else {
            continue;
        };
        if !dir.1.starts_with("sheet#") {
            continue;
        }
        let Some(png) = files.get(format!("{}/BINARY.png", dir.0).as_str()) else {
            continue;
        };
        sheets.push(SheetFile {
            xml_name: name.clone(),
            xml: String::from_utf8(data.clone()).map_err(|e| e.to_string())?,
            binary: decode_binary(png)?,
        });
    }

    // First pass: the score's confident digits become its own templates
    let mut prepared = Vec::new();
    let mut examples = Vec::new();
    for s in &sheets {
        let doc = parse(&s.xml)?;
        let data = read_sheet(&doc, s.binary.clone());
        let comps = candidate_components(&data);
        find_digits(&data, &comps, &[], &mut examples);
        prepared.push((data, comps));
    }
    let own = own_templates(examples);

    let mut report = DigitReport::default();
    let mut patched: HashMap<String, String> = HashMap::new();
    for (s, (data, comps)) in sheets.iter().zip(prepared) {
        let digits = find_digits(&data, &comps, &own, &mut Vec::new());
        report.digits += digits.len();
        let links = attach(&data, digits);
        report.attached += links.len();
        patched.insert(s.xml_name.clone(), patch_sheet(&s.xml, &data, &links)?);
    }

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in &entries {
        if name.ends_with('/') {
            out.add_directory(name.as_str(), options)
                .map_err(|e| e.to_string())?;
            continue;
        }
        out.start_file(name.as_str(), options)
            .map_err(|e| e.to_string())?;
        let data = patched
            .get(name)
            .map(|s| s.as_bytes())
            .unwrap_or(data.as_slice());
        std::io::Write::write_all(&mut out, data).map_err(|e| e.to_string())?;
    }
    let bytes = out.finish().map_err(|e| e.to_string())?.into_inner();
    Ok((bytes, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A template drawn back as a bitmap is read as its own digit
    #[test]
    fn templates_read_as_their_digit() {
        let data: &[u8] = include_bytes!("digit_templates.bin");
        let count = u16::from_le_bytes([data[4], data[5]]) as usize;
        assert!(count >= 50);
        let mut wrong = 0;
        let mut at = 8;
        for _ in 0..count {
            let digit = data[at];
            let pixels = &data[at + 5..at + 5 + TW * TH];
            at += 5 + TW * TH;
            // Twice the size, as a page would have it
            let mut m = Bitmap {
                w: TW * 2,
                h: TH * 2,
                px: vec![false; TW * TH * 4],
            };
            for y in 0..TH * 2 {
                for x in 0..TW * 2 {
                    m.set(x, y, pixels[(y / 2) * TW + x / 2] > 127);
                }
            }
            let (d, score, _, _, _) = classify(&m, &[]).unwrap();
            // Thin glyphs (1) lose some detail at this size; still above the threshold
            if d != digit || score < 0.62 {
                wrong += 1;
            }
        }
        assert_eq!(wrong, 0);
    }
}
