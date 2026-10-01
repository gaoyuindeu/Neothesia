//! The notes of the known score: where each is written on its staff

use roxmltree::{Document, Node, NodeId};

/// A printed note of the score
#[derive(Debug, Clone)]
pub struct Note {
    /// Staff counted over all parts, top to bottom (0 = first staff of the first part)
    pub staff: usize,
    /// Quarter notes from the start of the part, written order (no repeats)
    pub time: f64,
    pub grace: bool,
    /// Staff position: 0 bottom line, 1 the space above, ... 8 top line
    pub position: i32,
    pub node: NodeId,
    pub has_fingering: bool,
    /// The tuplet the note is in (an id over the score) and the number printed for it, if any
    pub tuplet: Option<(usize, Option<u8>)>,
    /// A staccatissimo / spiccato wedge printed at the note (a short upright stroke, like a 1)
    pub wedge: bool,
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(name))
}

fn text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|c| c.text()).map(str::trim)
}

/// Diatonic step index: C0 = 0, D0 = 1, ... C4 = 28
fn step_index(step: &str, octave: i32) -> Option<i32> {
    let s = match step {
        "C" => 0,
        "D" => 1,
        "E" => 2,
        "F" => 3,
        "G" => 4,
        "A" => 5,
        "B" => 6,
        _ => return None,
    };
    Some(octave * 7 + s)
}

/// Step index of the bottom line for a clef
fn bottom_line(sign: &str, line: i32, octave_change: i32) -> i32 {
    let clef_step = match sign {
        "G" => 32, // G4
        "F" => 24, // F3
        "C" => 28, // C4
        _ => 30,   // percussion and others: like treble
    };
    clef_step - 2 * (line - 1) + 7 * octave_change
}

/// Number of staves of each part and the notes of the score
pub fn notes(doc: &Document) -> (usize, Vec<Note>) {
    let mut out = Vec::new();
    let mut staff_base = 0;
    let mut tuplets = 0usize;
    for part in doc
        .root_element()
        .children()
        .filter(|c| c.has_tag_name("part"))
    {
        let mut staves = 1usize;
        let mut divisions = 1.0f64;
        // Per staff of the part: bottom line step, octave shift (in octaves)
        let mut bottom = [30i32; 8];
        let mut shift = [0i32; 8];
        let mut measure_start = 0.0f64;
        // Open tuplet of each voice: id, number printed
        let mut open: std::collections::HashMap<String, (usize, Option<u8>)> = Default::default();
        for measure in part.children().filter(|c| c.has_tag_name("measure")) {
            let mut pos = 0.0f64;
            let mut last_onset = 0.0f64;
            let mut longest = 0.0f64;
            for el in measure.children().filter(|c| c.is_element()) {
                match el.tag_name().name() {
                    "attributes" => {
                        if let Some(d) = text(el, "divisions").and_then(|d| d.parse::<f64>().ok())
                            && d > 0.0
                        {
                            divisions = d;
                        }
                        if let Some(s) = text(el, "staves").and_then(|s| s.parse().ok()) {
                            staves = s;
                        }
                        for clef in el.children().filter(|c| c.has_tag_name("clef")) {
                            let n: usize = clef
                                .attribute("number")
                                .and_then(|n| n.parse().ok())
                                .unwrap_or(1);
                            let sign = text(clef, "sign").unwrap_or("G");
                            let line: i32 = text(clef, "line")
                                .and_then(|l| l.parse().ok())
                                .unwrap_or(match sign {
                                    "F" => 4,
                                    "C" => 3,
                                    _ => 2,
                                });
                            let oc: i32 = text(clef, "clef-octave-change")
                                .and_then(|o| o.parse().ok())
                                .unwrap_or(0);
                            if (1..8).contains(&n) {
                                bottom[n] = bottom_line(sign, line, oc);
                            }
                        }
                    }
                    "direction" => {
                        let n: usize = text(el, "staff").and_then(|s| s.parse().ok()).unwrap_or(1);
                        for os in el.descendants().filter(|d| d.has_tag_name("octave-shift")) {
                            let size: i32 = os
                                .attribute("size")
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(8);
                            let octaves = if size >= 15 { 2 } else { 1 };
                            if (1..8).contains(&n) {
                                shift[n] = match os.attribute("type") {
                                    // 8va: written an octave below the sound
                                    Some("down") => octaves,
                                    Some("up") => -octaves,
                                    Some("stop") => 0,
                                    _ => shift[n],
                                };
                            }
                        }
                    }
                    "backup" => {
                        let d: f64 = text(el, "duration")
                            .and_then(|d| d.parse().ok())
                            .unwrap_or(0.0);
                        pos = (pos - d).max(0.0);
                    }
                    "forward" => {
                        let d: f64 = text(el, "duration")
                            .and_then(|d| d.parse().ok())
                            .unwrap_or(0.0);
                        pos += d;
                        longest = longest.max(pos);
                    }
                    "note" => {
                        let chord = child(el, "chord").is_some();
                        let grace = child(el, "grace").is_some();
                        let duration: f64 = if grace {
                            0.0
                        } else {
                            text(el, "duration")
                                .and_then(|d| d.parse().ok())
                                .unwrap_or(0.0)
                        };
                        let onset = if chord { last_onset } else { pos };
                        if !chord {
                            last_onset = pos;
                            pos += duration;
                            longest = longest.max(pos);
                        }
                        let voice = text(el, "voice").unwrap_or("1").to_string();
                        let marks: Vec<_> = el
                            .descendants()
                            .filter(|d| d.has_tag_name("tuplet"))
                            .collect();
                        if marks.iter().any(|t| t.attribute("type") == Some("start")) {
                            let start = marks
                                .iter()
                                .find(|t| t.attribute("type") == Some("start"))
                                .unwrap();
                            let number = child(el, "time-modification")
                                .and_then(|tm| text(tm, "actual-notes"))
                                .and_then(|n| n.parse::<u8>().ok())
                                .filter(|_| start.attribute("show-number") != Some("none"));
                            tuplets += 1;
                            open.insert(voice.clone(), (tuplets, number));
                        }
                        let tuplet = open.get(&voice).copied();
                        if marks.iter().any(|t| t.attribute("type") == Some("stop")) {
                            open.remove(&voice);
                        }
                        // Not printed: no head to find
                        if el.attribute("print-object") == Some("no") || child(el, "cue").is_some()
                        {
                            continue;
                        }
                        let n: usize = text(el, "staff")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(1)
                            .clamp(1, 7);
                        let Some(pitch) = child(el, "pitch") else {
                            continue;
                        };
                        let (Some(step), Some(octave)) = (
                            text(pitch, "step"),
                            text(pitch, "octave").and_then(|o| o.parse::<i32>().ok()),
                        ) else {
                            continue;
                        };
                        let Some(s) = step_index(step, octave) else {
                            continue;
                        };
                        let written = s - 7 * shift[n];
                        let has_fingering = el.descendants().any(|d| d.has_tag_name("fingering"));
                        out.push(Note {
                            staff: staff_base + n - 1,
                            time: measure_start + onset / divisions,
                            grace,
                            position: written - bottom[n],
                            node: el.id(),
                            has_fingering,
                            tuplet,
                            wedge: el.descendants().any(|d| {
                                d.has_tag_name("staccatissimo") || d.has_tag_name("spiccato")
                            }),
                        });
                    }
                    _ => {}
                }
            }
            measure_start += longest / divisions;
        }
        staff_base += staves;
    }
    (staff_base, out)
}
