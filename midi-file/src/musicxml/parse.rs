//! MusicXML (score-partwise) to a flat list of raw elements per measure.
//!
//! Only what the sheet and playback need is read; layout hints are ignored.

use std::{io::Read, path::Path};

use roxmltree::{Document, Node};

use crate::score::{Clef, Ornament};

#[derive(Debug, Clone, Default)]
pub struct RawNote {
    pub chord: bool,
    pub grace: bool,
    /// Slashed grace note (acciaccatura), played before the beat
    pub grace_slash: bool,
    pub rest: bool,
    pub measure_rest: bool,
    /// (letter 0 = C .. 6 = B, alter in semitones, octave)
    pub pitch: Option<(i32, f32, i32)>,
    /// In divisions
    pub duration: u64,
    pub voice: u32,
    /// 1-based staff within the part
    pub staff: usize,
    pub kind: Option<String>,
    pub dots: u8,
    pub tie_start: bool,
    pub tie_stop: bool,
    /// (actual, normal) notes
    pub time_modification: Option<(u32, u32)>,
    /// A tuplet starts here: bracket shown?
    pub tuplet_start: Option<Option<bool>>,
    pub tuplet_stop: bool,
    /// Value of the first beam: begin / continue / end / forward hook / backward hook
    pub beam: Option<String>,
    pub stem: Option<bool>,
    pub accidental: Option<String>,
    /// (start / stop / continue, number, placement above?)
    pub slurs: Vec<(String, u32, Option<bool>)>,
    pub staccato: bool,
    pub staccatissimo: bool,
    pub accent: bool,
    pub marcato: bool,
    pub tenuto: bool,
    pub fermata: bool,
    pub arpeggiate: bool,
    pub ornament: Option<Ornament>,
    pub fingering: Option<String>,
    pub printed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RawAttributes {
    pub divisions: Option<u64>,
    pub fifths: Option<i8>,
    pub time: Option<(u8, u8)>,
    pub staves: Option<usize>,
    /// (1-based staff, clef)
    pub clefs: Vec<(usize, Clef)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RawDirectionKind {
    Dynamic(String),
    Words {
        text: String,
        italic: bool,
        bold: bool,
    },
    /// crescendo / diminuendo / stop, number
    Wedge(String, u32),
    /// start / stop / change / continue
    Pedal(String),
    /// up / down / stop, size, number
    OctaveShift(String, u8, u32),
    Segno,
    Coda,
}

#[derive(Debug, Clone, Default)]
pub struct RawSound {
    pub tempo: Option<f64>,
    /// Loudness in percent of forte
    pub dynamics: Option<f64>,
    pub dacapo: bool,
    pub dalsegno: bool,
    pub tocoda: bool,
    pub fine: bool,
    pub segno: bool,
    pub coda: bool,
}

#[derive(Debug, Clone, Default)]
pub struct RawDirection {
    pub staff: usize,
    pub above: Option<bool>,
    /// Offset from the current position, in divisions
    pub offset: i64,
    pub kinds: Vec<RawDirectionKind>,
    pub sound: Option<RawSound>,
}

#[derive(Debug, Clone, Default)]
pub struct RawBarline {
    /// left / right
    pub location: String,
    /// forward / backward, times
    pub repeat: Option<(String, u32)>,
    /// (numbers, start / stop / discontinue)
    pub ending: Option<(String, String)>,
}

#[derive(Debug, Clone)]
pub enum RawElement {
    Note(Box<RawNote>),
    Backup(u64),
    Forward(u64),
    Attributes(RawAttributes),
    Direction(RawDirection),
    Barline(RawBarline),
    Sound(RawSound),
}

#[derive(Debug, Clone, Default)]
pub struct RawMeasure {
    pub number: String,
    /// Pickup or otherwise uncounted measure
    pub implicit: bool,
    pub elements: Vec<RawElement>,
}

#[derive(Debug, Clone, Default)]
pub struct RawPart {
    pub measures: Vec<RawMeasure>,
}

#[derive(Debug, Clone, Default)]
pub struct RawScore {
    pub title: Option<String>,
    pub parts: Vec<RawPart>,
}

/// Read a .musicxml / .xml file, or the root score of a compressed .mxl
pub fn read_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Could not open file: {e}"))?;
    let compressed = bytes.starts_with(b"PK");
    if !compressed {
        return String::from_utf8(bytes)
            .or_else(|e| {
                // UTF-16 files exist too
                let raw = e.into_bytes();
                decode_utf16(&raw).ok_or(())
            })
            .map_err(|_| "Could not decode the MusicXML text".to_string());
    }

    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Broken .mxl archive: {e}"))?;

    // META-INF/container.xml names the score; fall back to the first .xml file
    let mut root = None;
    if let Ok(mut container) = archive.by_name("META-INF/container.xml") {
        let mut text = String::new();
        container.read_to_string(&mut text).ok();
        if let Ok(doc) = Document::parse(&text) {
            root = doc
                .descendants()
                .find(|n| n.has_tag_name("rootfile"))
                .and_then(|n| n.attribute("full-path"))
                .map(str::to_string);
        }
    }
    let name = match root {
        Some(name) => name,
        None => (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| {
                (n.ends_with(".xml") || n.ends_with(".musicxml")) && !n.starts_with("META-INF")
            })
            .ok_or("No score inside the .mxl archive")?,
    };

    let mut file = archive
        .by_name(&name)
        .map_err(|e| format!("Broken .mxl archive: {e}"))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|e| format!("Broken .mxl archive: {e}"))?;
    String::from_utf8(raw.clone())
        .ok()
        .or_else(|| decode_utf16(&raw))
        .ok_or("Could not decode the MusicXML text".to_string())
}

fn decode_utf16(raw: &[u8]) -> Option<String> {
    let (le, body) = match raw {
        [0xFF, 0xFE, rest @ ..] => (true, rest),
        [0xFE, 0xFF, rest @ ..] => (false, rest),
        _ => return None,
    };
    let units: Vec<u16> = body
        .chunks_exact(2)
        .map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        })
        .collect();
    String::from_utf16(&units).ok()
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name(name))
}

fn children<'a, 'i>(node: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children().filter(move |n| n.has_tag_name(name))
}

fn text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|n| n.text()).map(str::trim)
}

fn number<T: std::str::FromStr>(node: Node, name: &str) -> Option<T> {
    text(node, name).and_then(|t| t.parse().ok())
}

fn placement(node: Node) -> Option<bool> {
    match node.attribute("placement") {
        Some("above") => Some(true),
        Some("below") => Some(false),
        _ => None,
    }
}

pub fn parse(xml: &str) -> Result<RawScore, String> {
    let doc = Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| format!("Invalid MusicXML: {e}"))?;
    let root = doc.root_element();
    if root.has_tag_name("score-timewise") {
        return Err("Time-wise MusicXML is not supported".into());
    }
    if !root.has_tag_name("score-partwise") {
        return Err("Not a MusicXML score".into());
    }

    let title = child(root, "work")
        .and_then(|w| text(w, "work-title"))
        .or_else(|| text(root, "movement-title"))
        .map(str::to_string);

    let parts = children(root, "part").map(parse_part).collect();
    Ok(RawScore { title, parts })
}

fn parse_part(part: Node) -> RawPart {
    let measures = children(part, "measure")
        .map(|m| RawMeasure {
            number: m.attribute("number").unwrap_or("").to_string(),
            implicit: m.attribute("implicit") == Some("yes"),
            elements: m.children().filter_map(parse_element).collect(),
        })
        .collect();
    RawPart { measures }
}

fn parse_element(node: Node) -> Option<RawElement> {
    Some(match node.tag_name().name() {
        "note" => RawElement::Note(Box::new(parse_note(node))),
        "backup" => RawElement::Backup(number(node, "duration").unwrap_or(0)),
        "forward" => RawElement::Forward(number(node, "duration").unwrap_or(0)),
        "attributes" => RawElement::Attributes(parse_attributes(node)),
        "direction" => RawElement::Direction(parse_direction(node)),
        "barline" => RawElement::Barline(parse_barline(node)),
        "sound" => RawElement::Sound(parse_sound(node)),
        _ => return None,
    })
}

fn parse_clef(node: Node) -> Clef {
    let sign = text(node, "sign").unwrap_or("G");
    let line: i32 = number(node, "line").unwrap_or(0);
    let octave: i32 = number(node, "clef-octave-change").unwrap_or(0);
    match (sign, line, octave) {
        ("F", _, -1) => Clef::Bass8vb,
        ("F", _, _) => Clef::Bass,
        ("C", 4, _) => Clef::Tenor,
        ("C", _, _) => Clef::Alto,
        ("G", _, -1) => Clef::Treble8vb,
        ("G", _, 1) => Clef::Treble8va,
        _ => Clef::Treble,
    }
}

fn parse_attributes(node: Node) -> RawAttributes {
    let time = child(node, "time").and_then(|t| {
        // Additive signatures ("3+2") are summed
        let beats: u32 = text(t, "beats")?
            .split('+')
            .filter_map(|b| b.trim().parse::<u32>().ok())
            .sum();
        let beat_type: u8 = number(t, "beat-type")?;
        Some((beats.min(255) as u8, beat_type))
    });
    RawAttributes {
        divisions: number(node, "divisions"),
        fifths: child(node, "key").and_then(|k| number(k, "fifths")),
        time,
        staves: number(node, "staves"),
        clefs: children(node, "clef")
            .map(|c| {
                let staff = c
                    .attribute("number")
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(1);
                (staff, parse_clef(c))
            })
            .collect(),
    }
}

fn parse_note(node: Node) -> RawNote {
    let mut note = RawNote {
        chord: child(node, "chord").is_some(),
        grace: child(node, "grace").is_some(),
        grace_slash: child(node, "grace").is_some_and(|g| g.attribute("slash") == Some("yes")),
        duration: number(node, "duration").unwrap_or(0),
        voice: number(node, "voice").unwrap_or(1),
        staff: number(node, "staff").unwrap_or(1),
        kind: text(node, "type").map(str::to_string),
        dots: children(node, "dot").count() as u8,
        stem: match text(node, "stem") {
            Some("up") => Some(true),
            Some("down") => Some(false),
            _ => None,
        },
        accidental: text(node, "accidental").map(str::to_string),
        printed: node.attribute("print-object") != Some("no"),
        ..Default::default()
    };

    if let Some(rest) = child(node, "rest") {
        note.rest = true;
        note.measure_rest = rest.attribute("measure") == Some("yes");
    }
    if let Some(pitch) = child(node, "pitch") {
        let letter = match text(pitch, "step").unwrap_or("C") {
            "C" => 0,
            "D" => 1,
            "E" => 2,
            "F" => 3,
            "G" => 4,
            "A" => 5,
            _ => 6,
        };
        note.pitch = Some((
            letter,
            number(pitch, "alter").unwrap_or(0.0),
            number(pitch, "octave").unwrap_or(4),
        ));
    } else if let Some(unpitched) = child(node, "unpitched") {
        // Percussion: place it on the display position
        let letter = match text(unpitched, "display-step").unwrap_or("B") {
            "C" => 0,
            "D" => 1,
            "E" => 2,
            "F" => 3,
            "G" => 4,
            "A" => 5,
            _ => 6,
        };
        note.pitch = Some((
            letter,
            0.0,
            number(unpitched, "display-octave").unwrap_or(4),
        ));
    }

    for tie in children(node, "tie") {
        match tie.attribute("type") {
            Some("start") => note.tie_start = true,
            Some("stop") => note.tie_stop = true,
            _ => {}
        }
    }
    if let Some(tm) = child(node, "time-modification") {
        note.time_modification = Some((
            number(tm, "actual-notes").unwrap_or(3),
            number(tm, "normal-notes").unwrap_or(2),
        ));
    }
    note.beam = children(node, "beam")
        .find(|b| b.attribute("number").unwrap_or("1") == "1")
        .and_then(|b| b.text())
        .map(|t| t.trim().to_string());

    for notations in children(node, "notations") {
        for n in notations.children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "tied" => match n.attribute("type") {
                    Some("start") => note.tie_start = true,
                    Some("stop") => note.tie_stop = true,
                    _ => {}
                },
                "slur" => note.slurs.push((
                    n.attribute("type").unwrap_or("start").to_string(),
                    n.attribute("number")
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(1),
                    placement(n),
                )),
                "tuplet" => match n.attribute("type") {
                    Some("start") => {
                        note.tuplet_start = Some(match n.attribute("bracket") {
                            Some("yes") => Some(true),
                            Some("no") => Some(false),
                            _ => None,
                        })
                    }
                    Some("stop") => note.tuplet_stop = true,
                    _ => {}
                },
                "fermata" => note.fermata = true,
                "arpeggiate" => note.arpeggiate = true,
                "articulations" => {
                    for a in n.children().filter(|a| a.is_element()) {
                        match a.tag_name().name() {
                            "staccato" => note.staccato = true,
                            "staccatissimo" | "spiccato" => note.staccatissimo = true,
                            "accent" => note.accent = true,
                            "strong-accent" => note.marcato = true,
                            "tenuto" => note.tenuto = true,
                            "detached-legato" => {
                                note.tenuto = true;
                                note.staccato = true;
                            }
                            _ => {}
                        }
                    }
                }
                "ornaments" => {
                    for o in n.children().filter(|o| o.is_element()) {
                        note.ornament = match o.tag_name().name() {
                            "trill-mark" => Some(Ornament::Trill),
                            "turn" | "delayed-turn" => Some(Ornament::Turn),
                            "inverted-turn" => Some(Ornament::InvertedTurn),
                            "mordent" => Some(Ornament::Mordent),
                            "inverted-mordent" => Some(Ornament::InvertedMordent),
                            _ => note.ornament,
                        };
                    }
                }
                "technical" => {
                    if let Some(f) = child(n, "fingering").and_then(|f| f.text()) {
                        note.fingering = Some(f.trim().to_string());
                    }
                }
                _ => {}
            }
        }
    }
    note
}

fn parse_sound(node: Node) -> RawSound {
    let num = |name: &str| node.attribute(name).and_then(|v| v.parse::<f64>().ok());
    RawSound {
        tempo: num("tempo"),
        dynamics: num("dynamics"),
        dacapo: node.attribute("dacapo") == Some("yes"),
        dalsegno: node.attribute("dalsegno").is_some(),
        tocoda: node.attribute("tocoda").is_some(),
        fine: node.attribute("fine").is_some(),
        segno: node.attribute("segno").is_some(),
        coda: node.attribute("coda").is_some(),
    }
}

fn parse_direction(node: Node) -> RawDirection {
    let mut direction = RawDirection {
        staff: number(node, "staff").unwrap_or(1),
        above: placement(node),
        offset: number(node, "offset").unwrap_or(0),
        sound: child(node, "sound").map(parse_sound),
        ..Default::default()
    };

    for dt in children(node, "direction-type") {
        for n in dt.children().filter(|n| n.is_element()) {
            let kind = match n.tag_name().name() {
                "dynamics" => {
                    let name: String = n
                        .children()
                        .filter(|c| c.is_element())
                        .map(|c| match c.tag_name().name() {
                            "other-dynamics" => c.text().unwrap_or("").trim().to_string(),
                            other => other.to_string(),
                        })
                        .collect();
                    if name.is_empty() {
                        continue;
                    }
                    RawDirectionKind::Dynamic(name)
                }
                "words" => {
                    let text = n.text().unwrap_or("").trim().to_string();
                    if text.is_empty() {
                        continue;
                    }
                    RawDirectionKind::Words {
                        text,
                        italic: n.attribute("font-style") == Some("italic"),
                        bold: n.attribute("font-weight") == Some("bold"),
                    }
                }
                "wedge" => RawDirectionKind::Wedge(
                    n.attribute("type").unwrap_or("stop").to_string(),
                    n.attribute("number")
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(1),
                ),
                "pedal" => {
                    RawDirectionKind::Pedal(n.attribute("type").unwrap_or("start").to_string())
                }
                "octave-shift" => RawDirectionKind::OctaveShift(
                    n.attribute("type").unwrap_or("stop").to_string(),
                    n.attribute("size")
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(8),
                    n.attribute("number")
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(1),
                ),
                "segno" => RawDirectionKind::Segno,
                "coda" => RawDirectionKind::Coda,
                _ => continue,
            };
            direction.kinds.push(kind);
        }
    }
    direction
}

fn parse_barline(node: Node) -> RawBarline {
    RawBarline {
        location: node.attribute("location").unwrap_or("right").to_string(),
        repeat: child(node, "repeat").map(|r| {
            (
                r.attribute("direction").unwrap_or("backward").to_string(),
                r.attribute("times")
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(2),
            )
        }),
        ending: child(node, "ending").map(|e| {
            (
                e.attribute("number").unwrap_or("1").to_string(),
                e.attribute("type").unwrap_or("start").to_string(),
            )
        }),
    }
}
