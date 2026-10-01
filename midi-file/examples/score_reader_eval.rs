//! How well the score reader gets fingering back from a printed score.
//!
//! The fingering is removed from a MusicXML score that has it, the printed score (PDF or
//! image) is read with the score reader, and the fingers written back are compared note by
//! note with the original.
//!
//! The original's fingering is made one finger per note first: several fingers written on
//! one note of a chord (one text "1\n3\n5" or several fingering elements) go to the notes
//! sounding with it on its staff, in the only order a hand can play them (upper staff: right
//! hand, fingers up with the pitch; lower staff: left hand, down); a group whose note count
//! does not match is left out of the comparison. A finger change ("5-4") on a note accepts
//! either finger.
//!
//! cargo run --release -p midi-file --example score_reader_eval -- <truth.mxl> <score.pdf>
//!
//! Development variables: SCORE_READER_LOST=<file> writes the lost fingers and the digits
//! read as JSON (for drawing them on the page).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use midi_file::musicxml::{read_file, score_reader, strip_fingering};
use roxmltree::{Document, NodeId};

/// The fingering texts of a note
fn fingering_texts(doc: &Document, note: NodeId) -> Vec<String> {
    doc.get_node(note)
        .unwrap()
        .descendants()
        .filter(|d| d.has_tag_name("fingering"))
        .filter_map(|f| f.text().map(str::to_string))
        .collect()
}

/// Truth of one note: the fingers accepted
type Accepted = Vec<u8>;

/// One finger (set) per note of the stripped score, from the original's fingering texts;
/// notes in `excluded` are not compared
fn truth_fingers(
    truth: &Document,
    stripped: &Document,
) -> (HashMap<NodeId, Accepted>, HashSet<NodeId>) {
    let elements = |d: &Document| -> Vec<NodeId> {
        d.descendants()
            .filter(|n| n.has_tag_name("note"))
            .map(|n| n.id())
            .collect()
    };
    let (t_notes, s_notes) = (elements(truth), elements(stripped));
    assert_eq!(t_notes.len(), s_notes.len(), "stripping changed the notes");

    // Notes sounding together on a staff, top to bottom
    let (_, placed) = score_reader::target::notes(stripped);
    let mut group_of: HashMap<NodeId, (usize, u64)> = HashMap::new();
    let mut groups: HashMap<(usize, u64), Vec<(i32, NodeId)>> = HashMap::new();
    for n in placed.iter().filter(|n| !n.grace) {
        let key = (n.staff, (n.time * 1e6).round() as u64);
        group_of.insert(n.node, key);
        groups.entry(key).or_default().push((n.position, n.node));
    }
    for g in groups.values_mut() {
        g.sort_by_key(|n| std::cmp::Reverse(n.0));
    }

    let mut fingers: HashMap<NodeId, Accepted> = HashMap::new();
    let mut excluded = HashSet::new();
    let pitch_of = |d: &Document, n: NodeId| -> String {
        d.get_node(n)
            .unwrap()
            .descendants()
            .filter(|c| {
                c.has_tag_name("step") || c.has_tag_name("octave") || c.has_tag_name("alter")
            })
            .filter_map(|c| c.text())
            .collect()
    };
    if std::env::var_os("SCORE_READER_CHECK").is_some() {
        let differ = t_notes
            .iter()
            .zip(&s_notes)
            .filter(|(t, s)| pitch_of(truth, **t) != pitch_of(stripped, **s))
            .count();
        eprintln!("notes whose pitch differs between truth and stripped: {differ}");
    }
    for (t, s) in t_notes.into_iter().zip(s_notes) {
        let texts = fingering_texts(truth, t);
        if texts.is_empty() {
            continue;
        }
        let mut stack: Vec<u8> = Vec::new();
        let mut change: Vec<u8> = Vec::new();
        for text in &texts {
            let digits: Vec<u8> = text
                .chars()
                .filter_map(|c| c.to_digit(10).map(|d| d as u8))
                .collect();
            // A change: digits joined by a dash, comma or tie mark
            if digits.len() > 1 && text.contains(['-', ',', '\u{361}', '~']) {
                change.extend(digits);
            } else {
                stack.extend(digits);
            }
        }
        stack.retain(|d| (1..=5).contains(d));
        change.retain(|d| (1..=5).contains(d));
        if !change.is_empty() && stack.is_empty() {
            fingers.insert(s, change);
            continue;
        }
        if std::env::var_os("SCORE_READER_CHECK").is_some() && stack.len() > 1 {
            let g = group_of.get(&s).and_then(|key| groups.get(key)).map(|g| {
                g.iter()
                    .map(|&(p, n)| (p, pitch_of(stripped, n)))
                    .collect::<Vec<_>>()
            });
            eprintln!(
                "stack {stack:?} on {} texts {texts:?} group {g:?}",
                pitch_of(stripped, s)
            );
        }
        match stack.len() {
            0 => {}
            1 => {
                fingers.insert(s, stack);
            }
            k => {
                // A chord's fingers written on one note: to its notes as a hand plays them
                let group = group_of.get(&s).and_then(|key| groups.get(key));
                match group {
                    Some(g) if g.len() == k => {
                        let lower_staff = stripped
                            .get_node(s)
                            .unwrap()
                            .children()
                            .find(|c| c.has_tag_name("staff"))
                            .and_then(|c| c.text())
                            .is_some_and(|t| t.trim() == "2");
                        let mut sorted = stack.clone();
                        // g is top to bottom: right hand fingers down from the top note
                        sorted.sort_unstable_by(|a, b| b.cmp(a));
                        if lower_staff {
                            sorted.reverse();
                        }
                        for (&(_, n), &d) in g.iter().zip(&sorted) {
                            fingers.insert(n, vec![d]);
                        }
                    }
                    Some(g) => excluded.extend(g.iter().map(|&(_, n)| n)),
                    None => {
                        excluded.insert(s);
                    }
                }
            }
        }
    }
    for n in &excluded {
        fingers.remove(n);
    }
    (fingers, excluded)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: score_reader_eval <truth.mxl> <score.pdf>");
        std::process::exit(2);
    }
    let truth_path = Path::new(&args[0]);
    let truth = read_file(truth_path).expect("ground truth");
    let stripped = strip_fingering(&truth).expect("strip");

    let t = std::time::Instant::now();
    let pages = score_reader::page::load(Path::new(&args[1])).expect("pages");
    if std::env::var_os("SCORE_READER_TIMING").is_some() {
        eprintln!("load {:.1?}", t.elapsed());
    }
    let (fingers, report, details) = score_reader::read_detailed(&stripped, &pages).expect("read");
    let seconds = t.elapsed().as_secs_f64();

    let opts = || roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let truth_doc = Document::parse_with_options(&truth, opts()).unwrap();
    let stripped_doc = Document::parse_with_options(&stripped, opts()).unwrap();
    let (truth_fingers, excluded) = truth_fingers(&truth_doc, &stripped_doc);
    let written: HashMap<NodeId, u8> = fingers.iter().copied().collect();
    let notes = stripped_doc
        .descendants()
        .filter(|n| n.has_tag_name("note"))
        .count();

    let (mut fingered, mut correct, mut wrong, mut missing, mut extra) = (0, 0, 0, 0, 0);
    // Why fingers were missed or wrong: the note not matched with a head, matched but no
    // digit given to it, or another digit given
    let (mut unaligned, mut no_digit, mut other_digit) = (0, 0, 0);
    let mut lost = Vec::new();
    let head_json = |n: &NodeId| {
        details
            .heads
            .get(n)
            .map_or("null".to_string(), |(p, b)| format!("[{p}, {b:?}]"))
    };
    for note in stripped_doc
        .descendants()
        .filter(|n| n.has_tag_name("note"))
    {
        let n = note.id();
        if excluded.contains(&n) {
            continue;
        }
        let got = written.get(&n).copied();
        let Some(accepted) = truth_fingers.get(&n) else {
            if let Some(d) = got {
                extra += 1;
                lost.push(format!(
                    r#"{{"kind": "extra", "finger": {d}, "head": {}}}"#,
                    head_json(&n)
                ));
            }
            continue;
        };
        fingered += 1;
        match got {
            Some(d) if accepted.contains(&d) => {
                correct += 1;
                continue;
            }
            Some(_) => wrong += 1,
            None => missing += 1,
        }
        let kind = if !details.aligned.contains(&n) {
            unaligned += 1;
            "unaligned"
        } else {
            match details.digits.get(&n) {
                Some(d) if !accepted.contains(d) => {
                    other_digit += 1;
                    "other digit"
                }
                _ => {
                    no_digit += 1;
                    "no digit"
                }
            }
        };
        let pitch = note
            .descendants()
            .find(|d| d.has_tag_name("pitch"))
            .map(|p| {
                let t = |name: &str| {
                    p.children()
                        .find(|c| c.has_tag_name(name))
                        .and_then(|c| c.text())
                        .unwrap_or("")
                        .to_string()
                };
                format!("{}{}{}", t("step"), t("alter"), t("octave"))
            })
            .unwrap_or_default();
        lost.push(format!(
            r#"{{"kind": "{kind}", "finger": {}, "pitch": "{pitch}", "measure": "{}", "head": {}}}"#,
            accepted[0],
            note.ancestors()
                .find(|a| a.has_tag_name("measure"))
                .and_then(|m| m.attribute("number"))
                .unwrap_or(""),
            head_json(&n)
        ));
    }

    if let Some(path) = std::env::var_os("SCORE_READER_LOST") {
        let digits: Vec<String> = details
            .digit_boxes
            .iter()
            .map(|(p, d, b, to)| {
                let head = to
                    .and_then(|n| details.heads.get(&n))
                    .map_or("null".to_string(), |(_, h)| format!("{h:?}"));
                format!(
                    r#"{{"page": {p}, "digit": {d}, "box": {b:?}, "attached": {}, "to": {head}}}"#,
                    to.is_some()
                )
            })
            .collect();
        let stages: Vec<String> = details
            .stages
            .iter()
            .map(|(s, p, d, b)| {
                format!(r#"{{"stage": "{s}", "page": {p}, "digit": {d}, "box": {b:?}}}"#)
            })
            .collect();
        std::fs::write(
            path,
            format!(
                "{{\"lost\": [{}], \"digits\": [{}], \"stages\": [{}]}}",
                lost.join(","),
                digits.join(","),
                stages.join(",")
            ),
        )
        .unwrap();
    }

    // Development: (stack, chord) pairs with their features and whether the stack's digits
    // are the chord's fingers, as JSON lines (training data of the association model)
    if let Some(path) = std::env::var_os("SCORE_READER_PAIRS") {
        let mut lines = String::new();
        for pair in &details.pairs {
            // Each digit of the stack to a different note of the chord that has it
            let mut used = vec![false; pair.notes.len()];
            let matched = pair
                .digits
                .iter()
                .filter(|&&d| {
                    let hit = pair.notes.iter().enumerate().position(|(k, n)| {
                        !used[k]
                            && n.and_then(|n| truth_fingers.get(&n))
                                .is_some_and(|a| a.contains(&d))
                    });
                    if let Some(k) = hit {
                        used[k] = true;
                    }
                    hit.is_some()
                })
                .count();
            let known = pair.notes.iter().any(|n| {
                n.is_some_and(|n| truth_fingers.contains_key(&n) || !excluded.contains(&n))
            });
            // The fingers of the chord's notes in the score (0: none)
            let truths: Vec<u8> = pair
                .notes
                .iter()
                .map(|n| n.and_then(|n| truth_fingers.get(&n)).map_or(0, |a| a[0]))
                .collect();
            // Stem (u / d / -) and voice of the chord's notes
            let child_text = |n: NodeId, name: &str| -> String {
                stripped_doc
                    .get_node(n)
                    .unwrap()
                    .children()
                    .find(|c| c.has_tag_name(name))
                    .and_then(|c| c.text())
                    .unwrap_or("")
                    .trim()
                    .to_string()
            };
            let stems: String = pair
                .notes
                .iter()
                .map(|n| match n.map(|n| child_text(n, "stem")).as_deref() {
                    Some("up") => 'u',
                    Some("down") => 'd',
                    _ => '-',
                })
                .collect();
            let voices: Vec<String> = pair
                .notes
                .iter()
                .map(|n| n.map(|n| child_text(n, "voice")).unwrap_or_default())
                .collect();
            lines.push_str(&format!(
                "{{\"page\": {}, \"stack\": {}, \"size\": {}, \"matched\": {}, \"known\": {}, \"features\": {:?}, \"boxes\": {:?}, \"heads\": {:?}, \"digits\": {:?}, \"truths\": {:?}, \"stems\": {stems:?}, \"voices\": {voices:?}}}\n",
                pair.page,
                pair.stack,
                pair.digits.len(),
                matched,
                known,
                pair.features,
                pair.boxes,
                pair.heads,
                pair.digits,
                truths
            ));
        }
        std::fs::write(path, lines).unwrap();
    }
    let written_n = correct + wrong + extra;
    println!(
        "{}: notes {} (heads found {}, aligned {}), fingered {} | digits read {} | written {} | correct {} wrong {} missing {} extra {} | precision {:.1}% recall {:.1}% ({:.1} s) | lost: unaligned {unaligned} no digit {no_digit} other digit {other_digit} | excluded {}",
        truth_path.file_name().unwrap().to_string_lossy(),
        notes,
        report.heads,
        report.aligned,
        fingered,
        report.digits,
        written_n,
        correct,
        wrong,
        missing,
        extra,
        100.0 * correct as f64 / written_n.max(1) as f64,
        100.0 * correct as f64 / fingered.max(1) as f64,
        seconds,
        excluded.len()
    );
}
