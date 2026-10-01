//! Reading the fingering of a printed score whose notes are already known.
//!
//! The music is known from the MusicXML file, so the page does not have to be
//! recognized as a whole: the staves are found, note heads and fingering digits are
//! detected, the heads of each staff are aligned with the notes of the score by their
//! staff positions, and the digits next to the heads give the notes their fingers.

pub mod align;
mod assign;
mod assoc;
pub mod detect;
mod dev;
mod gpu;
mod onnx;
pub mod page;
pub mod staves;
pub mod target;

use std::collections::HashMap;

use roxmltree::{Document, NodeId};

/// What a reading found
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    pub pages: usize,
    pub staves: usize,
    pub heads: usize,
    /// Heads matched with a note of the score
    pub aligned: usize,
    pub digits: usize,
    /// Digits attached to a head
    pub attached: usize,
    /// Fingers for notes of the score (attached digits on aligned heads)
    pub fingers: usize,
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn cx(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    fn cy(&self) -> f32 {
        (self.y0 + self.y1) / 2.0
    }
}

/// A head found on a page
struct Head {
    rect: Rect,
    /// System over all pages
    system: usize,
    /// Staff within its system (= staff of the score)
    staff: usize,
    position: f32,
}

/// Fingering digits and heads of one page
struct PageFound {
    interline: f32,
    digits: Vec<(u8, Rect)>,
    /// Indices into the list of all heads
    heads: Vec<usize>,
}

/// Heads within this many interlines horizontally are one chord
const CHORD_DX: f32 = 0.75;

/// Fingers for notes of the score (`(note element, finger)`), read from the pages
pub fn read(score: &str, pages: &[page::Gray]) -> Result<(Vec<(NodeId, u8)>, Report), String> {
    read_detailed(score, pages).map(|(fingers, report, _)| (fingers, report))
}

/// Development: which notes of the score a reading matched with a head, and the digits it
/// gave to notes (also notes that already had fingering)
#[derive(Debug, Default)]
pub struct Details {
    pub aligned: std::collections::HashSet<NodeId>,
    pub digits: HashMap<NodeId, u8>,
    /// Page and box (x0, y0, x1, y1) of the head matched with each note
    pub heads: HashMap<NodeId, (usize, [f32; 4])>,
    /// Every digit read: page, digit, box, the note it went to
    pub digit_boxes: Vec<(usize, u8, [f32; 4], Option<NodeId>)>,
    /// With SCORE_READER_PAIRS: every (stack, chord) pair considered, with its features
    pub pairs: Vec<PairOut>,
}

/// A (stack, chord) pair of a reading
#[derive(Debug)]
pub struct PairOut {
    pub page: usize,
    /// Number of the stack on its page
    pub stack: usize,
    /// The stack's digits, top to bottom
    pub digits: Vec<u8>,
    /// The chord's notes top to bottom (None: head not matched with a note)
    pub notes: Vec<Option<NodeId>>,
    pub features: Vec<f32>,
}

/// Fingers (note, finger), report and details of a reading
pub type Detailed = (Vec<(NodeId, u8)>, Report, Details);

/// `read`, with the details of what was matched
pub fn read_detailed(score: &str, pages: &[page::Gray]) -> Result<Detailed, String> {
    let doc = Document::parse_with_options(
        score,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("MusicXML: {e}"))?;
    let (staff_count, notes) = target::notes(&doc);
    let per_system = staff_count.max(1);
    let threshold = std::env::var("NEOTHESIA_DETECT_THRESHOLD")
        .ok()
        .and_then(|t| t.parse().ok())
        .unwrap_or(0.35);

    let mut report = Report {
        pages: pages.len(),
        ..Default::default()
    };
    let mut heads: Vec<Head> = Vec::new();
    let mut found_pages: Vec<PageFound> = Vec::new();
    let mut system_base = 0;
    // Development: pages written for an external detector, or its detections read back
    let pages_out = std::env::var_os("SCORE_READER_PAGES_OUT").map(std::path::PathBuf::from);
    let detections_in = std::env::var_os("SCORE_READER_DETECTIONS").map(std::path::PathBuf::from);
    // Development: time per stage (binarize, deskew, staves, detector)
    let timing = std::env::var_os("SCORE_READER_TIMING").is_some();
    let mut times = [std::time::Duration::ZERO; 4];
    for (page_no, gray) in pages.iter().enumerate() {
        let t = std::time::Instant::now();
        let bits = page::binarize(gray);
        times[0] += t.elapsed();
        let t = std::time::Instant::now();
        let bits = page::rotate(&bits, page::skew(&bits));
        times[1] += t.elapsed();
        let t = std::time::Instant::now();
        let Some((interline, thickness)) = staves::spacing(&bits) else {
            continue;
        };
        let found_staves = staves::find(&bits, interline, thickness);
        times[2] += t.elapsed();
        report.staves += found_staves.len();
        let systems = staves::systems(&bits, &found_staves, per_system);
        // staff index on the page -> (system, staff in system)
        let mut place = HashMap::new();
        for (s, system) in systems.iter().enumerate() {
            for (k, &i) in system.iter().enumerate() {
                place.insert(i, (system_base + s, k));
            }
        }
        system_base += systems.len();

        let t = std::time::Instant::now();
        let objects = if let Some(dir) = &pages_out {
            dev::write_page(dir, page_no, &bits, interline)?;
            Vec::new()
        } else if let Some(dir) = &detections_in {
            dev::read_detections(dir, page_no, threshold)?
        } else {
            detect::detect(bits.w, bits.h, &bits.px, interline, threshold)
                .ok_or("The detector could not run")?
        };
        times[3] += t.elapsed();
        let mut page_found = PageFound {
            interline,
            digits: Vec::new(),
            heads: Vec::new(),
        };
        for o in objects {
            let rect = Rect {
                x0: o.x0,
                y0: o.y0,
                x1: o.x1,
                y1: o.y1,
            };
            if o.class < detect::HEAD_BLACK {
                page_found
                    .digits
                    .push((o.class + 1, snap_to_ink(&bits, rect)));
                continue;
            }
            // The staff whose lines (with ledger lines) reach the head
            let cy = rect.cy();
            let staff = found_staves
                .iter()
                .enumerate()
                .filter(|(_, st)| {
                    let sp = st.spacing();
                    cy >= st.lines[0] - 6.0 * sp
                        && cy <= st.lines[4] + 6.0 * sp
                        && rect.cx() >= st.x0 as f32 - sp
                        && rect.cx() <= st.x1 as f32 + sp
                })
                .min_by(|a, b| {
                    let d = |st: &staves::Staff| (cy - (st.lines[0] + st.lines[4]) / 2.0).abs();
                    d(a.1).total_cmp(&d(b.1))
                });
            let Some((i, st)) = staff else {
                continue;
            };
            let Some(&(system, k)) = place.get(&i) else {
                continue;
            };
            page_found.heads.push(heads.len());
            heads.push(Head {
                rect,
                system,
                staff: k,
                position: st.position(cy),
            });
        }
        drop_numbers(&mut page_found, &heads, &found_staves, &place, interline);
        report.digits += page_found.digits.len();
        found_pages.push(page_found);
    }
    if timing {
        eprintln!(
            "binarize {:.1?}, deskew {:.1?}, staves {:.1?}, detector {:.1?}",
            times[0], times[1], times[2], times[3]
        );
    }
    report.heads = heads.len();
    let t = std::time::Instant::now();

    // Each staff of the score: found chords and written chords, aligned
    let mut note_of_head: Vec<Option<usize>> = vec![None; heads.len()];
    for k in 0..per_system {
        let found = found_chords(&heads, k, &found_pages);
        let written = written_chords(&notes, k);
        let found_pos: Vec<Vec<f32>> = found
            .iter()
            .map(|c| c.iter().map(|&h| heads[h].position).collect())
            .collect();
        let written_pos: Vec<Vec<i32>> = written
            .iter()
            .map(|c| c.iter().map(|&n| notes[n].position).collect())
            .collect();
        let pairs = align::align(&found_pos, &written_pos);
        if std::env::var_os("SCORE_READER_ALIGN_DEBUG").is_some() {
            debug_alignment(
                k,
                &heads,
                &found,
                &found_pos,
                &notes,
                &written,
                &written_pos,
                &pairs,
            );
        }
        for (i, j) in pairs {
            if align::chord_similarity(&found_pos[i], &written_pos[j]) < 0.5 {
                continue;
            }
            for (f, w) in align::pair_notes(&found_pos[i], &written_pos[j])
                .into_iter()
                .enumerate()
            {
                if let Some(w) = w {
                    note_of_head[found[i][f]] = Some(written[j][w]);
                }
            }
        }
    }
    report.aligned = note_of_head.iter().filter(|n| n.is_some()).count();
    let mut details = Details {
        aligned: note_of_head
            .iter()
            .flatten()
            .map(|&n| notes[n].node)
            .collect(),
        ..Default::default()
    };
    for (page_no, p) in found_pages.iter().enumerate() {
        for &h in &p.heads {
            if let Some(n) = note_of_head[h] {
                let r = heads[h].rect;
                details
                    .heads
                    .insert(notes[n].node, (page_no, [r.x0, r.y0, r.x1, r.y1]));
            }
        }
    }

    drop_tuplet_numbers(&mut found_pages, &heads, &notes, &note_of_head);
    drop_wedges(&mut found_pages, &heads, &notes, &note_of_head);

    let dump_pairs = std::env::var_os("SCORE_READER_PAIRS").is_some();
    // Digits next to heads, page by page
    let mut fingers = Vec::new();
    let debug = std::env::var_os("SCORE_READER_DEBUG").is_some();
    for (page_no, p) in found_pages.iter().enumerate() {
        if debug {
            for (d, r) in &p.digits {
                eprintln!(
                    "page {page_no} digit {d} {:.0} {:.0} {:.0} {:.0}",
                    r.x0, r.y0, r.x1, r.y1
                );
            }
            for &h in &p.heads {
                let r = heads[h].rect;
                eprintln!(
                    "page {page_no} head {h} {:.0} {:.0} {:.0} {:.0} staff {} system {} pos {:.1} note {:?}",
                    r.x0,
                    r.y0,
                    r.x1,
                    r.y1,
                    heads[h].staff,
                    heads[h].system,
                    heads[h].position,
                    note_of_head[h]
                );
            }
        }
        let chords = page_chords(&heads, &p.heads, p.interline);
        let mut pairs = Vec::new();
        let attached = attach(
            &p.digits,
            &heads,
            &chords,
            p.interline,
            per_system,
            dump_pairs.then_some(&mut pairs),
        );
        for pair in pairs {
            details.pairs.push(PairOut {
                page: page_no,
                stack: pair.id,
                digits: pair.stack.iter().map(|&d| p.digits[d].0).collect(),
                notes: pair
                    .chord
                    .iter()
                    .map(|&h| note_of_head[h].map(|n| notes[n].node))
                    .collect(),
                features: pair.features.to_vec(),
            });
        }
        for (i, (d, r)) in p.digits.iter().enumerate() {
            let to = attached
                .iter()
                .find(|a| a.0 == i)
                .and_then(|a| note_of_head[a.1])
                .map(|n| notes[n].node);
            details
                .digit_boxes
                .push((page_no, *d, [r.x0, r.y0, r.x1, r.y1], to));
        }
        for (digit, head) in attached {
            report.attached += 1;
            if let Some(n) = note_of_head[head] {
                fingers.push((notes[n].node, p.digits[digit].0, notes[n].has_fingering));
                details
                    .digits
                    .entry(notes[n].node)
                    .or_insert(p.digits[digit].0);
            }
        }
    }
    // One finger per note
    let mut seen = std::collections::HashSet::new();
    let fingers: Vec<(NodeId, u8)> = fingers
        .into_iter()
        .filter(|(node, _, printed)| !printed && seen.insert(*node))
        .map(|(node, d, _)| (node, d))
        .collect();
    report.fingers = fingers.len();
    if timing {
        eprintln!("align and attach {:.1?}", t.elapsed());
    }
    Ok((fingers, report, details))
}

/// The box of the ink inside a predicted box: detection boxes come from text boxes, taller
/// than the digit
fn snap_to_ink(bits: &page::Bitmap, r: Rect) -> Rect {
    let (x0, y0) = (r.x0.max(0.0) as usize, r.y0.max(0.0) as usize);
    let (x1, y1) = (
        (r.x1.max(0.0) as usize).min(bits.w.saturating_sub(1)),
        (r.y1.max(0.0) as usize).min(bits.h.saturating_sub(1)),
    );
    let (mut ix0, mut iy0, mut ix1, mut iy1) = (usize::MAX, usize::MAX, 0, 0);
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if bits.get(x, y) {
                n += 1;
                ix0 = ix0.min(x);
                iy0 = iy0.min(y);
                ix1 = ix1.max(x);
                iy1 = iy1.max(y);
            }
        }
    }
    if n < 6 {
        return r;
    }
    Rect {
        x0: ix0 as f32,
        y0: iy0 as f32,
        x1: (ix1 + 1) as f32,
        y1: (iy1 + 1) as f32,
    }
}

/// Digits that are not fingering: numbers of several digits side by side (measure numbers,
/// tempo marks) and numbers over the start of a system, before its first note (measure
/// numbers)
fn drop_numbers(
    page: &mut PageFound,
    heads: &[Head],
    staves: &[staves::Staff],
    place: &HashMap<usize, (usize, usize)>,
    il: f32,
) {
    let digits = &page.digits;
    let side_by_side = |a: &Rect, b: &Rect| {
        let overlap = a.y1.min(b.y1) - a.y0.max(b.y0);
        let gap = a.x0.max(b.x0) - a.x1.min(b.x1);
        overlap > 0.5 * (a.y1 - a.y0).min(b.y1 - b.y0) && gap < 0.35 * il
    };
    // Per system: left edge of its first head, top of its first staff
    let mut first_head: HashMap<usize, f32> = HashMap::new();
    for &h in &page.heads {
        let e = first_head.entry(heads[h].system).or_insert(f32::MAX);
        *e = e.min(heads[h].rect.x0);
    }
    let system_top = |cy: f32| -> Option<(usize, f32)> {
        // The system of the nearest staff below or around the digit
        staves
            .iter()
            .enumerate()
            .filter(|(_, st)| cy < st.lines[4])
            .min_by(|a, b| {
                (a.1.lines[0] - cy)
                    .abs()
                    .total_cmp(&(b.1.lines[0] - cy).abs())
            })
            .and_then(|(i, st)| {
                place
                    .get(&i)
                    .map(|&(system, k)| (system, if k == 0 { st.lines[0] } else { f32::MIN }))
            })
    };
    let keep: Vec<bool> = (0..digits.len())
        .map(|i| {
            let r = digits[i].1;
            if (0..digits.len()).any(|j| j != i && side_by_side(&r, &digits[j].1)) {
                return false;
            }
            if let Some((system, top)) = system_top(r.cy())
                && let Some(&x) = first_head.get(&system)
                && r.y1 <= top
                && r.cx() < x - 0.3 * il
            {
                return false;
            }
            true
        })
        .collect();
    let mut i = 0;
    page.digits.retain(|_| {
        i += 1;
        keep[i - 1]
    });
}

/// Development: the alignment of one staff, found chord by written chord ("=" paired, "-"
/// found only, "+" written only), with system / x and written time
#[allow(clippy::too_many_arguments)]
fn debug_alignment(
    k: usize,
    heads: &[Head],
    found: &[Vec<usize>],
    found_pos: &[Vec<f32>],
    notes: &[target::Note],
    written: &[Vec<usize>],
    written_pos: &[Vec<i32>],
    pairs: &[(usize, usize)],
) {
    let f = |i: usize| {
        let h = &heads[found[i][0]];
        let pos: Vec<String> = found_pos[i].iter().map(|p| format!("{p:.1}")).collect();
        format!("s{} x{:.0} [{}]", h.system, h.rect.cx(), pos.join(" "))
    };
    let w = |j: usize| {
        let n = &notes[written[j][0]];
        format!(
            "t{:.3}{} {:?}",
            n.time,
            if n.grace { "g" } else { "" },
            written_pos[j]
        )
    };
    let (mut i, mut j) = (0, 0);
    for &(pi, pj) in pairs
        .iter()
        .chain(std::iter::once(&(found.len(), written.len())))
    {
        while i < pi {
            eprintln!("A{k} - {}", f(i));
            i += 1;
        }
        while j < pj {
            eprintln!("A{k} + {}", w(j));
            j += 1;
        }
        if pi < found.len() && pj < written.len() {
            let s = align::chord_similarity(&found_pos[pi], &written_pos[pj]);
            eprintln!("A{k} = {} | {} sim {s:.2}", f(pi), w(pj));
            i += 1;
            j += 1;
        }
    }
}

/// Digits that are the number of a tuplet of the score: its number, over the middle of its
/// notes, away from their heads (beyond the stems or the bracket; fingering is close to the
/// notes)
fn drop_tuplet_numbers(
    pages: &mut [PageFound],
    heads: &[Head],
    notes: &[target::Note],
    note_of_head: &[Option<usize>],
) {
    // Heads of each printed tuplet number
    let mut groups: HashMap<usize, (u8, Vec<usize>)> = HashMap::new();
    for (h, n) in note_of_head.iter().enumerate() {
        if let Some(n) = n
            && let Some((id, Some(number))) = notes[*n].tuplet
        {
            groups.entry(id).or_insert((number, Vec::new())).1.push(h);
        }
    }
    for p in pages.iter_mut() {
        let il = p.interline;
        let on_page: std::collections::HashSet<usize> = p.heads.iter().copied().collect();
        let mut drop = vec![false; p.digits.len()];
        for (number, hs) in groups.values() {
            if !hs.iter().all(|h| on_page.contains(h)) || hs.is_empty() {
                continue;
            }
            let x0 = hs
                .iter()
                .map(|&h| heads[h].rect.x0)
                .fold(f32::MAX, f32::min);
            let x1 = hs
                .iter()
                .map(|&h| heads[h].rect.x1)
                .fold(f32::MIN, f32::max);
            let top = hs
                .iter()
                .map(|&h| heads[h].rect.y0)
                .fold(f32::MAX, f32::min);
            let bottom = hs
                .iter()
                .map(|&h| heads[h].rect.y1)
                .fold(f32::MIN, f32::max);
            let cx = (x0 + x1) / 2.0;
            for (i, (d, r)) in p.digits.iter().enumerate() {
                if d != number || (r.cx() - cx).abs() > 0.6 * il + 0.1 * (x1 - x0) {
                    continue;
                }
                let above = (top - r.y1) >= 1.8 * il && (top - r.y1) <= 7.0 * il;
                let below = (r.y0 - bottom) >= 1.8 * il && (r.y0 - bottom) <= 7.0 * il;
                if above || below {
                    drop[i] = true;
                }
            }
        }
        let mut i = 0;
        p.digits.retain(|_| {
            i += 1;
            !drop[i - 1]
        });
    }
}

/// Digits 1 that are the staccatissimo wedge of a note of the score: in the column of its
/// chord, close above or below the chord (the score has the wedge on one note of the chord,
/// the page at the chord's end)
fn drop_wedges(
    pages: &mut [PageFound],
    heads: &[Head],
    notes: &[target::Note],
    note_of_head: &[Option<usize>],
) {
    for p in pages.iter_mut() {
        let il = p.interline;
        // Vertical extent of the chord column of each wedged head
        let columns: Vec<(f32, f32, f32)> = p
            .heads
            .iter()
            .filter(|&&h| note_of_head[h].is_some_and(|n| notes[n].wedge))
            .map(|&h| {
                let (cx, mut top, mut bottom) =
                    (heads[h].rect.cx(), heads[h].rect.y0, heads[h].rect.y1);
                for &o in &p.heads {
                    let r = heads[o].rect;
                    if (heads[o].system, heads[o].staff) == (heads[h].system, heads[h].staff)
                        && (r.cx() - cx).abs() < 0.75 * il
                    {
                        top = top.min(r.y0);
                        bottom = bottom.max(r.y1);
                    }
                }
                (cx, top, bottom)
            })
            .collect();
        if columns.is_empty() {
            continue;
        }
        p.digits.retain(|(d, r)| {
            *d != 1
                || !columns.iter().any(|&(cx, top, bottom)| {
                    let gap = (top - r.y1).max(r.y0 - bottom);
                    (r.cx() - cx).abs() < 0.6 * il && gap < 2.5 * il
                })
        });
    }
}

/// Chords of found heads of staff `k`: systems in order, left to right
fn found_chords(heads: &[Head], k: usize, pages: &[PageFound]) -> Vec<Vec<usize>> {
    let mut idx: Vec<usize> = (0..heads.len()).filter(|&h| heads[h].staff == k).collect();
    idx.sort_by(|&a, &b| {
        heads[a]
            .system
            .cmp(&heads[b].system)
            .then(heads[a].rect.cx().total_cmp(&heads[b].rect.cx()))
    });
    let mut interline = vec![21.0f32; heads.len()];
    for p in pages {
        for &h in &p.heads {
            interline[h] = p.interline;
        }
    }
    let mut chords: Vec<Vec<usize>> = Vec::new();
    for h in idx {
        let join = chords.last().is_some_and(|c| {
            let first = &heads[c[0]];
            first.system == heads[h].system && same_chord(heads, c, h, interline[h])
        });
        if join {
            chords.last_mut().unwrap().push(h);
        } else {
            chords.push(vec![h]);
        }
    }
    chords
}

/// Whether head `h` belongs to the chord `chord` (heads of one staff): in its column, or
/// right next to it a second away (heads of a second sit on both sides of the stem,
/// touching; notes of a scale are further apart)
fn same_chord(heads: &[Head], chord: &[usize], h: usize, il: f32) -> bool {
    let dx = (heads[h].rect.cx() - heads[chord[0]].rect.cx()).abs();
    dx < CHORD_DX * il
        || (dx < 1.35 * il
            && chord
                .iter()
                .any(|&o| ((heads[o].position - heads[h].position).abs() - 1.0).abs() <= 0.35))
}

/// Chords of written notes of staff `k`, in time order (grace notes on their own)
fn written_chords(notes: &[target::Note], k: usize) -> Vec<Vec<usize>> {
    let mut idx: Vec<usize> = (0..notes.len()).filter(|&n| notes[n].staff == k).collect();
    idx.sort_by(|&a, &b| {
        notes[a]
            .time
            .total_cmp(&notes[b].time)
            .then(notes[b].grace.cmp(&notes[a].grace))
            .then(a.cmp(&b))
    });
    let mut chords: Vec<Vec<usize>> = Vec::new();
    for n in idx {
        let join = chords.last().is_some_and(|c| {
            let first = &notes[c[0]];
            !first.grace && !notes[n].grace && (first.time - notes[n].time).abs() < 1e-6
        });
        if join {
            chords.last_mut().unwrap().push(n);
        } else {
            chords.push(vec![n]);
        }
    }
    chords
}

/// Chords of the heads of one page, each top to bottom
fn page_chords(heads: &[Head], on_page: &[usize], interline: f32) -> Vec<Vec<usize>> {
    let mut idx = on_page.to_vec();
    idx.sort_by(|&a, &b| {
        (heads[a].system, heads[a].staff)
            .cmp(&(heads[b].system, heads[b].staff))
            .then(heads[a].rect.cx().total_cmp(&heads[b].rect.cx()))
    });
    let mut chords: Vec<Vec<usize>> = Vec::new();
    for h in idx {
        let join = chords.last().is_some_and(|c| {
            let first = &heads[c[0]];
            (first.system, first.staff) == (heads[h].system, heads[h].staff)
                && same_chord(heads, c, h, interline)
        });
        if join {
            chords.last_mut().unwrap().push(h);
        } else {
            chords.push(vec![h]);
        }
    }
    for c in &mut chords {
        c.sort_by(|&a, &b| heads[a].rect.cy().total_cmp(&heads[b].rect.cy()));
    }
    chords
}

/// (digit, head) pairs: stacked digits finger a chord top to bottom; a stack goes to the
/// chord it is best aligned with; a row of single digits at one height to one staff
/// A stack of digits and a chord it could belong to, with the features of the pair
/// (development: written out for training the association model)
pub struct Pair {
    /// Number of the stack on its page
    pub id: usize,
    /// Digits of the stack, top to bottom (indices into the page's digits)
    pub stack: Vec<usize>,
    /// Heads of the chord, top to bottom
    pub chord: Vec<usize>,
    pub features: [f32; assoc::FEATURES],
}

fn attach(
    digits: &[(u8, Rect)],
    heads: &[Head],
    chords: &[Vec<usize>],
    il: f32,
    per_system: usize,
    pairs_out: Option<&mut Vec<Pair>>,
) -> Vec<(usize, usize)> {
    let mut links = Vec::new();
    let mut used = vec![false; heads.len()];

    // A digit right before a head at its height, outside any chord's column: fingering
    // written left of the note (string instrument style, some piano editions for chords)
    // (only chords near it count: a note of the other staff may be straight above)
    let in_column = |r: &Rect| {
        chords.iter().any(|ch| {
            let hx0 = ch
                .iter()
                .map(|&h| heads[h].rect.x0)
                .fold(f32::MAX, f32::min);
            let hx1 = ch
                .iter()
                .map(|&h| heads[h].rect.x1)
                .fold(f32::MIN, f32::max);
            let top = heads[ch[0]].rect.y0;
            let bottom = heads[ch[ch.len() - 1]].rect.y1;
            (hx0 - 0.3 * il..=hx1 + 0.3 * il).contains(&r.cx())
                && r.cy() > top - 4.0 * il
                && r.cy() < bottom + 4.0 * il
        })
    };
    let mut beside = vec![false; digits.len()];
    for (d, (_, r)) in digits.iter().enumerate() {
        if in_column(r) {
            continue;
        }
        let head = chords
            .iter()
            .flatten()
            .copied()
            .filter(|&h| !used[h])
            .filter(|&h| {
                let hr = heads[h].rect;
                let gap = hr.x0 - r.x1;
                (hr.cy() - r.cy()).abs() < 0.6 * il && (-0.2 * il..=1.2 * il).contains(&gap)
            })
            .min_by(|&a, &b| (heads[a].rect.x0 - r.x1).total_cmp(&(heads[b].rect.x0 - r.x1)));
        if let Some(h) = head {
            used[h] = true;
            beside[d] = true;
            links.push((d, h));
        }
    }

    // Stacks: digits above one another
    let mut order: Vec<usize> = (0..digits.len()).filter(|&d| !beside[d]).collect();
    order.sort_by(|&a, &b| digits[a].1.y0.total_cmp(&digits[b].1.y0));
    let mut stacks: Vec<Vec<usize>> = Vec::new();
    for d in order {
        let r = digits[d].1;
        let home = stacks.iter_mut().find(|st| {
            let last = digits[*st.last().unwrap()].1;
            let dy = r.cy() - last.cy();
            (r.cx() - last.cx()).abs() < 0.6 * il && (0.6 * il..=2.2 * il).contains(&dy)
        });
        match home {
            Some(st) => st.push(d),
            None => stacks.push(vec![d]),
        }
    }

    // The chords a stack could belong to, best first: (score, chord, above?, staff key)
    type Candidate = (f32, usize, bool, (usize, usize));
    let candidates = |st: &[usize]| -> Vec<Candidate> {
        let scx = st.iter().map(|&d| digits[d].1.cx()).sum::<f32>() / st.len() as f32;
        let (sy0, sy1) = (digits[st[0]].1.y0, digits[st[st.len() - 1]].1.y1);
        let (scy0, scy1) = (digits[st[0]].1.cy(), digits[st[st.len() - 1]].1.cy());
        let mut out = Vec::new();
        for (c, ch) in chords.iter().enumerate() {
            let hx0 = ch
                .iter()
                .map(|&h| heads[h].rect.x0)
                .fold(f32::MAX, f32::min);
            let hx1 = ch
                .iter()
                .map(|&h| heads[h].rect.x1)
                .fold(f32::MIN, f32::max);
            if !(hx0 - 0.7 * il..=hx1 + 0.7 * il).contains(&scx) {
                continue;
            }
            let top = heads[ch[0]].rect.y0;
            let bottom = heads[ch[ch.len() - 1]].rect.y1;
            // Above or below the chord; digits set tight to a chord may overlap its end head
            let (dist, above) = if scy1 < top || sy1 <= top + 0.5 * il {
                ((top - sy1).max(0.0), true)
            } else if scy0 > bottom || sy0 >= bottom - 0.5 * il {
                ((sy0 - bottom).max(0.0), false)
            } else {
                continue;
            };
            if dist > 10.0 * il {
                continue;
            }
            let score = 2.0 * (scx - (hx0 + hx1) / 2.0).abs() / il + 0.3 * dist / il;
            out.push((score, c, above, (heads[ch[0]].system, heads[ch[0]].staff)));
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    };
    let cands: Vec<Vec<Candidate>> = stacks.iter().map(|st| candidates(st)).collect();

    // Features of each (stack, candidate chord) pair
    let stack_cx =
        |st: &[usize]| st.iter().map(|&d| digits[d].1.cx()).sum::<f32>() / st.len() as f32;
    let stack_cy =
        |st: &[usize]| st.iter().map(|&d| digits[d].1.cy()).sum::<f32>() / st.len() as f32;
    let row_of: Vec<Vec<usize>> = (0..stacks.len())
        .map(|i| {
            (0..stacks.len())
                .filter(|&j| {
                    j != i
                        && (stack_cy(&stacks[i]) - stack_cy(&stacks[j])).abs() < 0.6 * il
                        && (stack_cx(&stacks[i]) - stack_cx(&stacks[j])).abs() < 8.0 * il
                })
                .collect()
        })
        .collect();
    let features = |i: usize, k: usize| -> [f32; assoc::FEATURES] {
        let st = &stacks[i];
        let (score, c, above, key) = cands[i][k];
        let ch = &chords[c];
        let hx0 = ch
            .iter()
            .map(|&h| heads[h].rect.x0)
            .fold(f32::MAX, f32::min);
        let hx1 = ch
            .iter()
            .map(|&h| heads[h].rect.x1)
            .fold(f32::MIN, f32::max);
        let (top, bottom) = (heads[ch[0]].rect.y0, heads[ch[ch.len() - 1]].rect.y1);
        let (scx, scy) = (stack_cx(st), stack_cy(st));
        let dist = if above {
            top - digits[st[st.len() - 1]].1.y1
        } else {
            digits[st[0]].1.y0 - bottom
        };
        let height = st
            .iter()
            .map(|&d| digits[d].1.y1 - digits[d].1.y0)
            .sum::<f32>()
            / st.len() as f32;
        // Bottom line of the chord's staff, from a head and its staff position
        let h0 = &heads[ch[0]];
        let staff_bottom = h0.rect.cy() + h0.position * il / 2.0;
        let lower_staff = per_system >= 2 && h0.staff == per_system - 1;
        let other_staff = cands[i].iter().filter(|o| o.3 != key).count();
        let row = &row_of[i];
        let agree = if row.is_empty() {
            0.5
        } else {
            row.iter()
                .filter(|&&j| cands[j].first().is_some_and(|b| b.3 == key))
                .count() as f32
                / row.len() as f32
        };
        [
            (scx - (hx0 + hx1) / 2.0) / il,
            (scx - (hx0 + hx1) / 2.0).abs() / il,
            dist.max(0.0) / il,
            above as u8 as f32,
            st.len() as f32,
            ch.len() as f32,
            st.len() as f32 - ch.len() as f32,
            k as f32,
            cands[i].len() as f32,
            score - cands[i][0].0,
            height / il,
            lower_staff as u8 as f32,
            ((staff_bottom - scy) / (il / 2.0)).clamp(-40.0, 40.0) / 10.0,
            h0.position / 10.0,
            heads[ch[ch.len() - 1]].position / 10.0,
            other_staff as f32,
            (hx1 - hx0) / il,
            score,
            row.len() as f32 / 5.0,
            agree,
        ]
    };
    let feats: Vec<Vec<[f32; assoc::FEATURES]>> = (0..stacks.len())
        .map(|i| (0..cands[i].len()).map(|k| features(i, k)).collect())
        .collect();
    if let Some(out) = pairs_out {
        for (i, st) in stacks.iter().enumerate() {
            for (k, cand) in cands[i].iter().enumerate() {
                out.push(Pair {
                    id: i,
                    stack: st.clone(),
                    chord: chords[cand.1].clone(),
                    features: feats[i][k],
                });
            }
        }
    }
    let model = assoc::model();

    // Each stack to a side (above / below) of a chord, all stacks of the page at once with the
    // least total cost: a row of digits between two staves goes to the staff whose notes no
    // other row can take. A stack can stay without a chord (cost UNASSIGNED).
    // With the model: cost -ln p, a stack stays alone below probability P0
    let p0: f64 = std::env::var("SCORE_READER_P0")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0.03);
    let unassigned: f64 = if model.is_some() { -p0.ln() } else { 8.0 };
    const FORBIDDEN: f64 = 1e6;
    // Columns: a single note takes one digit (from above or below), a chord one stack from
    // above and one from below
    let n = stacks.len();
    let mut slot_of = Vec::with_capacity(chords.len());
    let mut slots: Vec<(usize, Option<bool>)> = Vec::new();
    let one_slot = std::env::var_os("SCORE_READER_ONE_SLOT").is_some();
    let two_sided = |c: usize| chords[c].len() > 1 && !one_slot;
    for c in 0..chords.len() {
        slot_of.push(slots.len());
        if !two_sided(c) {
            slots.push((c, None));
        } else {
            slots.push((c, Some(true)));
            slots.push((c, Some(false)));
        }
    }
    let real = slots.len();
    let cols = real + n;
    let mut cost = vec![FORBIDDEN; n * cols];
    for (i, cand) in cands.iter().enumerate() {
        for (k, &(score, chord, above, _)) in cand.iter().enumerate() {
            let slot = slot_of[chord] + usize::from(two_sided(chord) && !above);
            // More digits than notes: a worse fit
            let excess = stacks[i].len().saturating_sub(chords[chord].len()) as f64;
            cost[i * cols + slot] = match model {
                Some(m) => -(m.prob(&feats[i][k]) as f64).max(1e-4).ln(),
                None => score as f64 + excess,
            };
        }
        cost[i * cols + real + i] = unassigned;
    }
    // A chord with stacks from above and below that hold more digits than it has notes: the
    // worse of the two is not allowed there, and the page assigned again
    let mut choice = Vec::new();
    for _ in 0..8 {
        if n == 0 {
            break;
        }
        choice = assign::assign(&cost, n, cols);
        let mut on_chord: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, &slot) in choice.iter().enumerate() {
            if slot < real && cost[i * cols + slot] < FORBIDDEN {
                on_chord.entry(slots[slot].0).or_default().push(i);
            }
        }
        let mut changed = false;
        for (c, st) in on_chord {
            let digits_on: usize = st.iter().map(|&i| stacks[i].len()).sum();
            if st.len() < 2 || digits_on <= chords[c].len() {
                continue;
            }
            let worst = *st
                .iter()
                .max_by(|&&a, &&b| {
                    cost[a * cols + choice[a]].total_cmp(&cost[b * cols + choice[b]])
                })
                .unwrap();
            cost[worst * cols + choice[worst]] = FORBIDDEN;
            changed = true;
        }
        if !changed {
            break;
        }
    }

    for (i, st) in stacks.iter().enumerate() {
        let slot = choice[i];
        if slot >= real || cost[i * cols + slot] >= FORBIDDEN {
            continue;
        }
        let chord = slots[slot].0;
        // The side the stack is on
        let Some(&(_, _, above, _)) = cands[i].iter().find(|c| c.1 == chord) else {
            continue;
        };
        let ch = &chords[chord];
        let mut st = st.clone();
        if st.len() > ch.len() {
            st = if above {
                st.split_off(st.len() - ch.len())
            } else {
                st.truncate(ch.len());
                st
            };
        }
        let targets: Vec<usize> = if above {
            ch[..st.len()].to_vec()
        } else {
            ch[ch.len() - st.len()..].to_vec()
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

/// The MusicXML text with `fingers` (note element, finger) added
pub fn insert_fingers(xml: &str, fingers: &[(NodeId, u8)]) -> Result<String, String> {
    super::fingering_transfer::insert_fingers(xml, fingers)
}

/// Read the fingering of the printed score `input` (PDF or image) into the MusicXML file at
/// `score` (keeping a copy of the original next to it)
pub fn read_into_file(score: &std::path::Path, input: &std::path::Path) -> Result<Report, String> {
    let xml = super::read_file(score)?;
    let pages = page::load(input)?;
    let (fingers, report) = read(&xml, &pages)?;
    if !fingers.is_empty() {
        let text = super::fingering_transfer::insert_fingers(&xml, &fingers)?;
        super::fingering_transfer::write_score(score, &text)?;
    }
    Ok(report)
}
