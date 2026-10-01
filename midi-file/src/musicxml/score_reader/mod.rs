//! Reading the fingering of a printed score whose notes are already known.
//!
//! The music is known from the MusicXML file, so the page does not have to be
//! recognized as a whole: the staves are found, note heads and fingering digits are
//! detected, the heads of each staff are aligned with the notes of the score by their
//! staff positions, and the digits next to the heads give the notes their fingers.

pub mod align;
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
        let systems = staves::systems(&found_staves, per_system);
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
        for (i, j) in align::align(&found_pos, &written_pos) {
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
        for (digit, head) in attach(&p.digits, &heads, &chords, p.interline) {
            report.attached += 1;
            if let Some(n) = note_of_head[head] {
                fingers.push((notes[n].node, p.digits[digit].0, notes[n].has_fingering));
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
    Ok((fingers, report))
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
fn attach(
    digits: &[(u8, Rect)],
    heads: &[Head],
    chords: &[Vec<usize>],
    il: f32,
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
            let (dist, above) = if scy1 < top {
                ((top - sy1).max(0.0), true)
            } else if scy0 > bottom {
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

    // Rows of single digits vote for one staff
    let mut singles: Vec<usize> = (0..stacks.len())
        .filter(|&i| stacks[i].len() == 1 && !cands[i].is_empty())
        .collect();
    singles.sort_by(|&a, &b| {
        digits[stacks[a][0]]
            .1
            .y0
            .total_cmp(&digits[stacks[b][0]].1.y0)
    });
    let mut rows: Vec<Vec<usize>> = Vec::new();
    for i in singles {
        let d = digits[stacks[i][0]].1;
        let row = rows.iter_mut().find(|row| {
            let r = digits[stacks[*row.last().unwrap()][0]].1;
            (d.y0 - r.y0).abs() < 0.6 * il && (d.x0 - r.x0).abs() < 6.0 * il
        });
        match row {
            Some(row) => row.push(i),
            None => rows.push(vec![i]),
        }
    }
    let mut staff_of: HashMap<usize, (usize, usize)> = HashMap::new();
    for row in rows.iter().filter(|r| r.len() >= 3) {
        let mut votes: HashMap<(usize, usize), usize> = HashMap::new();
        for &i in row {
            *votes.entry(cands[i][0].3).or_default() += 1;
        }
        let staff = votes
            .into_iter()
            .max_by_key(|(s, n)| (*n, std::cmp::Reverse(*s)))
            .unwrap()
            .0;
        for &i in row {
            staff_of.insert(i, staff);
        }
    }

    for (i, st) in stacks.iter().enumerate() {
        let options: Vec<&Candidate> = match staff_of.get(&i) {
            Some(staff) => {
                let o: Vec<_> = cands[i].iter().filter(|c| c.3 == *staff).collect();
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
