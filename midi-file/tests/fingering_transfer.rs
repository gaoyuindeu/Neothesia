//! Moving fingering from a recognized edition into a score

use midi_file::{
    Hand,
    musicxml::{TransferReport, load_str, transfer_fingering, transfer_fingering_into_file},
};

fn score(divisions: u32, measures: &[String]) -> String {
    let mut body = String::new();
    for (i, m) in measures.iter().enumerate() {
        let attributes = if i == 0 {
            format!(
                "<attributes><divisions>{divisions}</divisions><time><beats>4</beats><beat-type>4</beat-type></time>\
                 <staves>2</staves><clef number=\"1\"><sign>G</sign><line>2</line></clef>\
                 <clef number=\"2\"><sign>F</sign><line>4</line></clef></attributes>"
            )
        } else {
            String::new()
        };
        body.push_str(&format!(
            "<measure number=\"{}\">{attributes}{m}</measure>\n",
            i + 1
        ));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list><score-part id="P1"><part-name>Piano</part-name></score-part></part-list>
  <part id="P1">
{body}  </part>
</score-partwise>"#
    )
}

/// `pitch` like "C4", "F#4"; `finger` 0 for none; `extra` goes inside <note>
fn note(pitch: &str, duration: u32, staff: u32, chord: bool, finger: u8, extra: &str) -> String {
    let (step, rest) = pitch.split_at(1);
    let (alter, octave) = match rest.strip_prefix('#') {
        Some(o) => ("<alter>1</alter>", o),
        None => match rest.strip_prefix('b') {
            Some(o) => ("<alter>-1</alter>", o),
            None => ("", rest),
        },
    };
    let fingering = if finger > 0 {
        format!("<notations><technical><fingering>{finger}</fingering></technical></notations>")
    } else {
        String::new()
    };
    format!(
        "<note>{}<pitch><step>{step}</step>{alter}<octave>{octave}</octave></pitch>\
         <duration>{duration}</duration><voice>{staff}</voice><type>quarter</type><staff>{staff}</staff>{fingering}{extra}</note>",
        if chord { "<chord/>" } else { "" }
    )
}

fn backup(duration: u32) -> String {
    format!("<backup><duration>{duration}</duration></backup>")
}

/// The target: right hand C D E F | G A B C, left hand C3+G3 half notes
fn target() -> String {
    let rh1 = ["C4", "D4", "E4", "F4"]
        .map(|p| note(p, 1, 1, false, 0, ""))
        .concat();
    // The score already prints a 5 on the last C
    let rh2 = [("G4", 0), ("A4", 0), ("B4", 0), ("C5", 5)]
        .map(|(p, f)| note(p, 1, 1, false, f, ""))
        .concat();
    let lh = format!(
        "{}{}{}{}",
        note("C3", 2, 2, false, 0, ""),
        note("G3", 2, 2, true, 0, ""),
        note("C3", 2, 2, false, 0, ""),
        note("G3", 2, 2, true, 0, "<lyric><text>la</text></lyric>"),
    );
    score(
        1,
        &[
            format!("{rh1}{}{lh}", backup(4)),
            format!("{rh2}{}{lh}", backup(4)),
        ],
    )
}

/// What a recognizer might make of a fingered edition: other divisions, all in one
/// measure, the F read as F#, the A missed, an extra note, and fingers everywhere
fn recognized() -> String {
    let rh = [
        ("C4", 1),
        ("D4", 2),
        ("E4", 3),
        ("F#4", 1),
        ("G4", 2),
        ("D5", 0),
        ("B4", 4),
        ("C5", 3),
    ]
    .map(|(p, f)| note(p, 2, 1, false, f, ""))
    .concat();
    let lh = [(1, 1), (2, 2)]
        .repeat(2)
        .into_iter()
        .map(|(_, top)| {
            format!(
                "{}{}",
                note("C3", 4, 2, false, 5, ""),
                note("G3", 4, 2, true, top, "")
            )
        })
        .collect::<String>();
    score(2, &[format!("{rh}{}{lh}", backup(16))])
}

fn fingers(xml: &str, hand: Hand) -> Vec<(u8, Option<String>)> {
    let file = load_str("t.musicxml", xml).unwrap();
    let score = file.score.as_ref().unwrap();
    let staff = if hand == Hand::Right { 0 } else { 1 };
    score
        .measures
        .iter()
        .flat_map(|m| m.staves[staff].voices.iter().flat_map(|v| v.events.iter()))
        .flat_map(|e| e.notes.iter())
        .map(|n| (n.pitch, n.fingering.clone()))
        .collect()
}

#[test]
fn fingers_land_on_the_matching_notes() {
    let (out, report) = transfer_fingering(&target(), &[recognized()]).unwrap();
    assert_eq!(
        report,
        TransferReport {
            // The extra D5 has no finger
            found: 15,
            matched: 15,
            written: 14,
            kept: 1,
        }
    );

    let f = |d: &str| Some(d.to_string());
    assert_eq!(
        fingers(&out, Hand::Right),
        vec![
            (60, f("1")),
            (62, f("2")),
            (64, f("3")),
            // Read as F#: still this note
            (65, f("1")),
            (67, f("2")),
            // Missed by the recognizer
            (69, None),
            (71, f("4")),
            // Printed in the score, kept
            (72, f("5")),
        ]
    );
    assert_eq!(
        fingers(&out, Hand::Left),
        vec![
            (48, f("5")),
            (55, f("1")),
            (48, f("5")),
            (55, f("2")),
            (48, f("5")),
            (55, f("1")),
            (48, f("5")),
            (55, f("2")),
        ]
    );

    // Only additions: removing them gives the original back
    let stripped = out
        .replace(
            "<notations><technical><fingering>1</fingering></technical></notations>",
            "",
        )
        .replace(
            "<notations><technical><fingering>2</fingering></technical></notations>",
            "",
        )
        .replace(
            "<notations><technical><fingering>3</fingering></technical></notations>",
            "",
        )
        .replace(
            "<notations><technical><fingering>4</fingering></technical></notations>",
            "",
        )
        .replace(
            "<notations><technical><fingering>5</fingering></technical></notations>",
            "",
        );
    let original = target().replace(
        "<notations><technical><fingering>5</fingering></technical></notations>",
        "",
    );
    assert_eq!(stripped, original);
    // The lyric stays after the new notations
    assert!(out.contains("</notations><lyric>"));
}

#[test]
fn existing_notations_get_a_technical_element() {
    let target = score(
        1,
        &[format!(
            "{}{}",
            note("C4", 2, 1, false, 0, ""),
            note("E4", 2, 1, false, 0, "")
        )
        .replace(
            "<staff>1</staff></note>",
            "<staff>1</staff><notations><slur type=\"start\"/></notations></note>",
        )],
    );
    let source = score(
        1,
        &[format!(
            "{}{}",
            note("C4", 2, 1, false, 1, ""),
            note("E4", 2, 1, false, 3, "")
        )],
    );
    let (out, report) = transfer_fingering(&target, &[source]).unwrap();
    assert_eq!(report.written, 2);
    assert!(out.contains(
        "<notations><technical><fingering>1</fingering></technical><slur type=\"start\"/></notations>"
    ));
    let f = fingers(&out, Hand::Right);
    assert_eq!(f[1], (64, Some("3".into())));
}

#[test]
fn compressed_scores_are_rewritten_with_a_backup() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("neothesia_transfer_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("song.mxl");

    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("META-INF/container.xml", options).unwrap();
    zip.write_all(
        br#"<container><rootfiles><rootfile full-path="score.xml"/></rootfiles></container>"#,
    )
    .unwrap();
    zip.start_file("score.xml", options).unwrap();
    zip.write_all(target().as_bytes()).unwrap();
    std::fs::write(&path, zip.finish().unwrap().into_inner()).unwrap();

    let report = transfer_fingering_into_file(&path, &[recognized()]).unwrap();
    assert_eq!(report.written, 14);
    assert!(dir.join("song.mxl.bak").exists());

    let file = midi_file::MidiFile::new(&path).unwrap();
    let right = file
        .tracks
        .iter()
        .find(|t| t.hand == Some(Hand::Right))
        .unwrap();
    let printed: Vec<bool> = right
        .notes
        .iter()
        .map(|n| n.finger.unwrap().printed)
        .collect();
    // All but the missed A are printed now
    assert_eq!(
        printed,
        vec![true, true, true, true, true, false, true, true]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
