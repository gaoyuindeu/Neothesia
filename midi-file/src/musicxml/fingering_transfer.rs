//! Copy fingering from one MusicXML score into another.
//!
//! The source is usually optical music recognition output of a printed edition (a PDF):
//! its notes are approximately those of the target, with recognition errors (missing or
//! extra notes, wrong accidentals or octaves, measures split or merged). Both scores are
//! turned into one chord sequence per staff, the sequences are aligned with
//! Needleman-Wunsch on pitch similarity, and the fingers of aligned chords are moved to
//! the target notes of the same pitch (or the same rank in the chord).
//!
//! The target text is only added to: a `<fingering>` element goes into each note that
//! gets a finger, everything else stays byte for byte.

use std::{io::Read, ops::Range, path::Path};

use roxmltree::{Document, Node, NodeId};

/// MusicXML files usually start with a DOCTYPE
fn parse_xml(xml: &str) -> Result<Document<'_>, roxmltree::Error> {
    Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
}

/// What a transfer did
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransferReport {
    /// Fingered notes in the source
    pub found: usize,
    /// Of those, placed on a target note
    pub matched: usize,
    /// Written into the target (matched notes that had no fingering yet)
    pub written: usize,
    /// Matched notes that already had a fingering, left as they were
    pub kept: usize,
}

/// A note of a score, with where it sits
#[derive(Debug, Clone)]
struct ScoreNote {
    /// Index of the (part, staff) stream
    stream: usize,
    /// Position in quarter notes from the start of the part (written order, no repeats)
    time: f64,
    grace: bool,
    pitch: i32,
    fingering: Option<u8>,
    /// Byte range of the `<note>` element
    range: Range<usize>,
    node: Option<NodeId>,
}

fn child<'a, 'i>(node: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|c| c.has_tag_name(name))
}

fn text<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    child(node, name).and_then(|c| c.text()).map(str::trim)
}

fn midi_pitch(pitch: Node) -> Option<i32> {
    let step = match text(pitch, "step")? {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => return None,
    };
    let alter = text(pitch, "alter")
        .and_then(|a| a.parse::<f32>().ok())
        .unwrap_or(0.0)
        .round() as i32;
    let octave: i32 = text(pitch, "octave")?.parse().ok()?;
    Some((octave + 1) * 12 + step + alter)
}

fn note_fingering(note: Node) -> Option<u8> {
    let technical = child(child(note, "notations")?, "technical")?;
    technical
        .children()
        .filter(|c| c.has_tag_name("fingering"))
        .find_map(|f| crate::fingering::printed_digit(f.text().unwrap_or("")))
}

/// Every pitched note, streams numbered in (part, staff) order
fn collect(doc: &Document) -> (Vec<ScoreNote>, usize) {
    let mut notes = Vec::new();
    let mut streams: Vec<(usize, usize)> = Vec::new();
    let mut stream_of = |part: usize, staff: usize| {
        if let Some(i) = streams.iter().position(|s| *s == (part, staff)) {
            i
        } else {
            streams.push((part, staff));
            streams.len() - 1
        }
    };

    let root = doc.root_element();
    for (part_index, part) in root
        .children()
        .filter(|c| c.has_tag_name("part"))
        .enumerate()
    {
        let mut divisions = 1.0f64;
        // Start of the current measure, in quarters
        let mut measure_start = 0.0f64;
        for measure in part.children().filter(|c| c.has_tag_name("measure")) {
            // Position in the measure, in divisions
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
                        let staff: usize =
                            text(el, "staff").and_then(|s| s.parse().ok()).unwrap_or(1);
                        if let Some(pitch) = child(el, "pitch").and_then(midi_pitch) {
                            notes.push(ScoreNote {
                                stream: stream_of(part_index, staff),
                                time: measure_start + onset / divisions,
                                grace,
                                pitch,
                                fingering: note_fingering(el),
                                range: el.range(),
                                node: Some(el.id()),
                            });
                        }
                    }
                    _ => {}
                }
            }
            measure_start += longest / divisions;
        }
    }
    (notes, streams.len())
}

/// Notes sounding together in one stream (grace notes on their own)
#[derive(Debug)]
struct Chord {
    /// Indices into the note list, sorted by pitch
    notes: Vec<usize>,
}

fn chords(notes: &[ScoreNote], stream: usize) -> Vec<Chord> {
    let mut idx: Vec<usize> = (0..notes.len())
        .filter(|&i| notes[i].stream == stream)
        .collect();
    idx.sort_by(|&a, &b| {
        let (na, nb) = (&notes[a], &notes[b]);
        na.time
            .total_cmp(&nb.time)
            .then(nb.grace.cmp(&na.grace))
            .then(na.range.start.cmp(&nb.range.start))
    });
    let mut out: Vec<Chord> = Vec::new();
    for i in idx {
        let same = out.last().is_some_and(|c| {
            let first = &notes[c.notes[0]];
            !first.grace && !notes[i].grace && (first.time - notes[i].time).abs() < 1e-6
        });
        if same {
            out.last_mut().unwrap().notes.push(i);
        } else {
            out.push(Chord { notes: vec![i] });
        }
    }
    for chord in &mut out {
        chord.notes.sort_by_key(|&i| notes[i].pitch);
    }
    out
}

/// How alike two pitches are, allowing for typical recognition errors
fn pitch_similarity(a: i32, b: i32) -> f64 {
    match (a - b).abs() {
        0 => 1.0,
        // Missed or extra accidental, or a head on the neighbouring line / space
        1 | 2 => 0.5,
        // Octave line / clef errors
        12 => 0.4,
        _ => 0.0,
    }
}

/// 0..1: best pairing of the pitches of two chords, relative to the larger chord
fn chord_similarity(sn: &[ScoreNote], a: &Chord, tn: &[ScoreNote], b: &Chord) -> f64 {
    let pa: Vec<i32> = a.notes.iter().map(|&i| sn[i].pitch).collect();
    let pb: Vec<i32> = b.notes.iter().map(|&i| tn[i].pitch).collect();
    let mut used = vec![false; pb.len()];
    let mut total = 0.0;
    for &p in &pa {
        let best = (0..pb.len())
            .filter(|&j| !used[j])
            .map(|j| (j, pitch_similarity(p, pb[j])))
            .max_by(|x, y| x.1.total_cmp(&y.1));
        if let Some((j, s)) = best
            && s > 0.0
        {
            used[j] = true;
            total += s;
        }
    }
    total / pa.len().max(pb.len()) as f64
}

/// Needleman-Wunsch: pairs (source chord, target chord) of the best alignment
fn align(
    sn: &[ScoreNote],
    a: &[Chord],
    tn: &[ScoreNote],
    b: &[Chord],
) -> (Vec<(usize, usize)>, f64) {
    const GAP: f64 = -0.3;
    // A pairing scores its similarity, shifted so that unlike chords cost
    let pair = |i: usize, j: usize| chord_similarity(sn, &a[i], tn, &b[j]) * 2.0 - 0.8;

    let (n, m) = (a.len(), b.len());
    let w = m + 1;
    let mut score = vec![0.0f64; (n + 1) * w];
    // 0 diagonal, 1 skip source, 2 skip target
    let mut from = vec![0u8; (n + 1) * w];
    for i in 1..=n {
        score[i * w] = GAP * i as f64;
        from[i * w] = 1;
    }
    for j in 1..=m {
        score[j] = GAP * j as f64;
        from[j] = 2;
    }
    for i in 1..=n {
        for j in 1..=m {
            let diag = score[(i - 1) * w + j - 1] + pair(i - 1, j - 1);
            let up = score[(i - 1) * w + j] + GAP;
            let left = score[i * w + j - 1] + GAP;
            let (best, dir) = if diag >= up && diag >= left {
                (diag, 0)
            } else if up >= left {
                (up, 1)
            } else {
                (left, 2)
            };
            score[i * w + j] = best;
            from[i * w + j] = dir;
        }
    }

    let mut pairs = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        match from[i * w + j] {
            0 if i > 0 && j > 0 => {
                pairs.push((i - 1, j - 1));
                i -= 1;
                j -= 1;
            }
            1 if i > 0 => i -= 1,
            _ if j > 0 => j -= 1,
            _ => i -= 1,
        }
    }
    pairs.reverse();
    (pairs, score[n * w + m])
}

/// For each fingered note of a source chord, the target note of the paired chord
fn place_fingers(
    sn: &[ScoreNote],
    a: &Chord,
    tn: &[ScoreNote],
    b: &Chord,
    out: &mut Vec<(usize, u8)>,
) {
    let same_size = a.notes.len() == b.notes.len();
    let mut used = vec![false; b.notes.len()];
    for (rank, &i) in a.notes.iter().enumerate() {
        let Some(finger) = sn[i].fingering else {
            continue;
        };
        let p = sn[i].pitch;
        // Equal pitches (unisons of two voices): the one at the same place in the chord
        let by_pitch = (0..b.notes.len())
            .filter(|&j| !used[j])
            .map(|j| (j, pitch_similarity(p, tn[b.notes[j]].pitch)))
            .filter(|(_, s)| *s > 0.0)
            .max_by(|x, y| {
                x.1.total_cmp(&y.1)
                    .then(y.0.abs_diff(rank).cmp(&x.0.abs_diff(rank)))
            })
            .map(|(j, _)| j);
        // Same number of notes: the same place in the chord is a safe guess
        let target = by_pitch.or((same_size && !used[rank]).then_some(rank));
        if let Some(j) = target {
            used[j] = true;
            out.push((b.notes[j], finger));
        }
    }
}

/// Insert `<fingering>` into the `<note>` element `node`
fn insertion(xml: &str, doc: &Document, node: NodeId, finger: u8) -> Option<(usize, String)> {
    let note = doc.get_node(node)?;
    let fingering = format!("<fingering>{finger}</fingering>");
    let after_start_tag = |node: Node| -> Option<usize> {
        let start = node.range().start;
        Some(start + xml[start..].find('>')? + 1)
    };
    if let Some(notations) = child(note, "notations") {
        if let Some(technical) = child(notations, "technical") {
            let r = technical.range();
            if xml[r.clone()].trim_end().ends_with("/>") {
                // <technical/>: rewrite as an element with content
                return None;
            }
            return Some((after_start_tag(technical)?, fingering));
        }
        if xml[notations.range()].trim_end().ends_with("/>") {
            return None;
        }
        return Some((
            after_start_tag(notations)?,
            format!("<technical>{fingering}</technical>"),
        ));
    }
    // <notations> goes before these, at the end of <note> otherwise
    let at = note
        .children()
        .find(|c| matches!(c.tag_name().name(), "lyric" | "play" | "listen"))
        .map(|c| c.range().start)
        .unwrap_or_else(|| {
            let r = note.range();
            r.start + xml[r.clone()].rfind("</").unwrap_or(r.end - r.start)
        });
    Some((
        at,
        format!("<notations><technical>{fingering}</technical></notations>"),
    ))
}

/// Copy the fingering of the `sources` (in order, e.g. the movements of a recognized PDF)
/// into `target`. Returns the new target text.
pub fn transfer(target: &str, sources: &[String]) -> Result<(String, TransferReport), String> {
    let target_doc = parse_xml(target).map_err(|e| format!("Target MusicXML: {e}"))?;
    let (tn, t_streams) = collect(&target_doc);

    // All sources as one score: later movements continue in time
    let mut sn: Vec<ScoreNote> = Vec::new();
    let mut s_streams = 0;
    let mut offset = 0.0;
    for source in sources {
        let doc = parse_xml(source).map_err(|e| format!("Recognized MusicXML: {e}"))?;
        let (notes, streams) = collect(&doc);
        let end = notes.iter().map(|n| n.time).fold(0.0, f64::max);
        sn.extend(notes.into_iter().map(|mut n| {
            n.time += offset;
            n.node = None;
            n
        }));
        offset += end + 1000.0;
        s_streams = s_streams.max(streams);
    }

    let mut report = TransferReport {
        found: sn.iter().filter(|n| n.fingering.is_some()).count(),
        ..Default::default()
    };
    if report.found == 0 {
        return Ok((target.to_string(), report));
    }

    let s_chords: Vec<Vec<Chord>> = (0..s_streams).map(|s| chords(&sn, s)).collect();
    let t_chords: Vec<Vec<Chord>> = (0..t_streams).map(|s| chords(&tn, s)).collect();

    // Pair the streams by how well they align (usually upper staff with upper staff)
    // (alignment score, source stream, target stream, chord pairs)
    type Candidate = (f64, usize, usize, Vec<(usize, usize)>);
    let mut candidates: Vec<Candidate> = Vec::new();
    for (s, a) in s_chords.iter().enumerate() {
        if !a
            .iter()
            .flat_map(|c| &c.notes)
            .any(|&i| sn[i].fingering.is_some())
        {
            continue;
        }
        for (t, b) in t_chords.iter().enumerate() {
            let (pairs, score) = align(&sn, a, &tn, b);
            candidates.push((score, s, t, pairs));
        }
    }
    candidates.sort_by(|x, y| y.0.total_cmp(&x.0));
    let mut used_s = vec![false; s_streams];
    let mut used_t = vec![false; t_streams];
    let mut placed: Vec<(usize, u8)> = Vec::new();
    for (score, s, t, pairs) in candidates {
        if used_s[s] || used_t[t] || score <= 0.0 {
            continue;
        }
        used_s[s] = true;
        used_t[t] = true;
        for (i, j) in pairs {
            let (a, b) = (&s_chords[s][i], &t_chords[t][j]);
            // Only chords that really correspond
            if chord_similarity(&sn, a, &tn, b) >= 0.5 {
                place_fingers(&sn, a, &tn, b, &mut placed);
            }
        }
    }
    report.matched = placed.len();

    let mut edits: Vec<(usize, String)> = Vec::new();
    for (note, finger) in placed {
        if tn[note].fingering.is_some() {
            report.kept += 1;
            continue;
        }
        let edit = tn[note]
            .node
            .and_then(|node| insertion(target, &target_doc, node, finger));
        if let Some(edit) = edit {
            edits.push(edit);
            report.written += 1;
        }
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = target.to_string();
    for (at, text) in edits {
        out.insert_str(at, &text);
    }
    Ok((out, report))
}

/// Read the score text of a .musicxml / .xml file or of a compressed .mxl
fn read_any(path: &Path) -> Result<String, String> {
    super::read_file(path)
}

/// Write fingering from the `sources` into the MusicXML file at `path`, keeping a copy of
/// the original next to it (`<name>.bak`, made once)
pub fn transfer_into_file(path: &Path, sources: &[String]) -> Result<TransferReport, String> {
    let target = read_any(path)?;
    let (text, report) = transfer(&target, sources)?;
    if report.written > 0 {
        write_score(path, &text)?;
    }
    Ok(report)
}

/// `fingers` (note element, finger) added to the MusicXML text
pub(crate) fn insert_fingers(xml: &str, fingers: &[(NodeId, u8)]) -> Result<String, String> {
    let doc = parse_xml(xml).map_err(|e| e.to_string())?;
    let mut edits: Vec<(usize, String)> = fingers
        .iter()
        .filter_map(|&(node, finger)| insertion(xml, &doc, node, finger))
        .collect();
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = xml.to_string();
    for (at, text) in edits {
        out.insert_str(at, &text);
    }
    Ok(out)
}

/// Replace the score at `path` by `text` (inside its archive for .mxl), keeping a copy of
/// the original next to it (`<name>.bak`, made once)
pub(crate) fn write_score(path: &Path, text: &str) -> Result<(), String> {
    let mut backup = path.as_os_str().to_owned();
    backup.push(".bak");
    let backup = std::path::PathBuf::from(backup);
    if !backup.exists() {
        std::fs::copy(path, &backup).map_err(|e| format!("Could not back up the score: {e}"))?;
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.starts_with(b"PK") {
        let out = rewrite_mxl(&bytes, text)?;
        std::fs::write(path, out).map_err(|e| format!("Could not write the score: {e}"))?;
    } else {
        std::fs::write(path, text).map_err(|e| format!("Could not write the score: {e}"))?;
    }
    Ok(())
}

/// The archive with its root score replaced by `text`
fn rewrite_mxl(bytes: &[u8], text: &str) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("Broken .mxl archive: {e}"))?;

    let mut root = None;
    if let Ok(mut container) = archive.by_name("META-INF/container.xml") {
        let mut s = String::new();
        container.read_to_string(&mut s).ok();
        if let Ok(doc) = Document::parse(&s) {
            root = doc
                .descendants()
                .find(|n| n.has_tag_name("rootfile"))
                .and_then(|n| n.attribute("full-path"))
                .map(str::to_string);
        }
    }
    let root = match root {
        Some(r) => r,
        None => (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| {
                (n.ends_with(".xml") || n.ends_with(".musicxml")) && !n.starts_with("META-INF")
            })
            .ok_or("No score inside the .mxl archive")?,
    };

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).map_err(|e| e.to_string())?;
        if name == root {
            data = text.as_bytes().to_vec();
        }
        // "mimetype" must stay uncompressed and first
        let options = if name == "mimetype" {
            options.compression_method(zip::CompressionMethod::Stored)
        } else {
            options
        };
        out.start_file(name, options).map_err(|e| e.to_string())?;
        std::io::Write::write_all(&mut out, &data).map_err(|e| e.to_string())?;
    }
    Ok(out.finish().map_err(|e| e.to_string())?.into_inner())
}

/// The text without `<fingering>` elements (and the `<technical>` / `<notations>` they
/// leave empty): a score to test the transfer on
pub fn strip_fingering(xml: &str) -> Result<String, String> {
    let doc = parse_xml(xml).map_err(|e| e.to_string())?;
    let only_fingering = |node: Node| {
        node.children()
            .filter(|c| c.is_element())
            .all(|c| c.has_tag_name("fingering"))
    };
    let mut cuts: Vec<Range<usize>> = Vec::new();
    for fingering in doc.descendants().filter(|n| n.has_tag_name("fingering")) {
        let mut cut = fingering;
        if let Some(technical) = cut.parent().filter(|p| p.has_tag_name("technical"))
            && only_fingering(technical)
        {
            cut = technical;
            if let Some(notations) = cut.parent().filter(|p| p.has_tag_name("notations"))
                && notations
                    .children()
                    .filter(|c| c.is_element())
                    .all(|c| c == technical)
            {
                cut = notations;
            }
        }
        cuts.push(cut.range());
    }
    cuts.sort_by_key(|r| r.start);
    cuts.dedup();
    let mut out = xml.to_string();
    for r in cuts.into_iter().rev() {
        out.replace_range(r, "");
    }
    Ok(out)
}

/// Pitch and fingering of every pitched note, in document order
pub fn note_fingerings(xml: &str) -> Result<Vec<(i32, Option<u8>)>, String> {
    let doc = parse_xml(xml).map_err(|e| e.to_string())?;
    let (notes, _) = collect(&doc);
    let mut notes: Vec<&ScoreNote> = notes.iter().collect();
    notes.sort_by_key(|n| n.range.start);
    Ok(notes.iter().map(|n| (n.pitch, n.fingering)).collect())
}
