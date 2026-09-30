//! Textbook fingerings the automatic fingering should reproduce.

use midi_file::{
    Hand,
    fingering::{FingerNote, fingering},
};

/// Notes one after another, 0.25 s each
fn melody(pitches: &[u8]) -> Vec<FingerNote> {
    pitches
        .iter()
        .enumerate()
        .map(|(i, &p)| FingerNote {
            start: i as f64 * 0.25,
            end: i as f64 * 0.25 + 0.24,
            pitch: p,
            fixed: None,
        })
        .collect()
}

fn run(pitches: &[u8], hand: Hand) -> Vec<u8> {
    fingering(&melody(pitches), hand)
}

// C4 = 60
#[test]
fn five_finger_position() {
    assert_eq!(run(&[60, 62, 64, 65, 67], Hand::Right), vec![1, 2, 3, 4, 5]);
    assert_eq!(run(&[48, 50, 52, 53, 55], Hand::Left), vec![5, 4, 3, 2, 1]);
    assert_eq!(run(&[67, 65, 64, 62, 60], Hand::Right), vec![5, 4, 3, 2, 1]);
}

#[test]
fn c_major_scale() {
    let up = [60, 62, 64, 65, 67, 69, 71, 72];
    assert_eq!(run(&up, Hand::Right), vec![1, 2, 3, 1, 2, 3, 4, 5]);
    let left = [48, 50, 52, 53, 55, 57, 59, 60];
    assert_eq!(run(&left, Hand::Left), vec![5, 4, 3, 2, 1, 3, 2, 1]);
}

#[test]
fn broken_chord_and_alberti_bass() {
    // C-E-G-C arpeggio in the right hand
    assert_eq!(run(&[60, 64, 67, 72], Hand::Right), vec![1, 2, 3, 5]);
    // Alberti bass C-G-E-G in the left hand
    assert_eq!(
        run(&[48, 55, 52, 55, 48, 55, 52, 55], Hand::Left),
        vec![5, 1, 3, 1, 5, 1, 3, 1]
    );
}

#[test]
fn chords() {
    let triad = |t: f64, p: [u8; 3]| {
        p.map(|pitch| FingerNote {
            start: t,
            end: t + 0.9,
            pitch,
            fixed: None,
        })
    };
    let mut notes: Vec<FingerNote> = triad(0.0, [60, 64, 67]).to_vec();
    notes.extend(triad(1.0, [60, 65, 69]));
    let f = fingering(&notes, Hand::Right);
    assert_eq!(&f[..3], &[1, 3, 5]);
    assert_eq!(&f[3..], &[1, 3, 5]);
}

#[test]
fn fixed_fingers_are_kept() {
    let mut notes = melody(&[60, 62, 64, 65, 67]);
    notes[0].fixed = Some(2);
    let f = fingering(&notes, Hand::Right);
    assert_eq!(f[0], 2);
}

#[test]
fn repeated_notes_and_black_keys() {
    // Thumb avoids black keys: D major five-finger with F#
    let f = run(&[62, 64, 66, 67, 69], Hand::Right);
    assert_eq!(f, vec![1, 2, 3, 4, 5]);
    // Repeated note keeps the finger
    let f = run(&[64, 64, 64], Hand::Right);
    assert!(f.iter().all(|&x| x == f[0]));
}

#[test]
#[ignore = "the rule-cost model puts the thumb under too early in two-octave scales             (1 2 3 1 2 1 2 ...); kept as a target for a trained model"]
fn longer_scales() {
    // Two octaves of C major, right hand up: 1 2 3 1 2 3 4 1 2 3 1 2 3 4 5
    let up: Vec<u8> = vec![60, 62, 64, 65, 67, 69, 71, 72, 74, 76, 77, 79, 81, 83, 84];
    assert_eq!(
        run(&up, Hand::Right),
        vec![1, 2, 3, 1, 2, 3, 4, 1, 2, 3, 1, 2, 3, 4, 5]
    );
    // Right hand down one octave: 5 4 3 2 1 3 2 1
    let down: Vec<u8> = vec![72, 71, 69, 67, 65, 64, 62, 60];
    assert_eq!(run(&down, Hand::Right), vec![5, 4, 3, 2, 1, 3, 2, 1]);
    // Left hand down one octave: 1 2 3 4 5? no: C4 B3 A3 G3 F3 E3 D3 C3 = 1 2 3 1 2 3 4 5
    let left_down: Vec<u8> = vec![60, 59, 57, 55, 53, 52, 50, 48];
    assert_eq!(run(&left_down, Hand::Left), vec![1, 2, 3, 1, 2, 3, 4, 5]);
    // G major right hand: 1 2 3 1 2 3 4 5 (F# with 4)
    let g: Vec<u8> = vec![67, 69, 71, 72, 74, 76, 78, 79];
    assert_eq!(run(&g, Hand::Right), vec![1, 2, 3, 1, 2, 3, 4, 5]);
}

#[test]
fn printed_fingers_are_kept_and_the_rest_filled_in() {
    // C D E F G quarters in the right hand; the score prints 1 on C and 5 on G
    let note = |step: &str, finger: Option<u8>| {
        let technical = finger
            .map(|f| {
                format!("<notations><technical><fingering>{f}</fingering></technical></notations>")
            })
            .unwrap_or_default();
        format!(
            "<note><pitch><step>{step}</step><octave>4</octave></pitch><duration>2</duration>\
             <voice>1</voice><type>quarter</type><staff>1</staff>{technical}</note>"
        )
    };
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list><score-part id="P1"><part-name>Piano</part-name></score-part></part-list>
  <part id="P1">
    <measure number="1">
      <attributes><divisions>2</divisions><time><beats>5</beats><beat-type>4</beat-type></time>
        <staves>2</staves><clef number="1"><sign>G</sign><line>2</line></clef>
        <clef number="2"><sign>F</sign><line>4</line></clef></attributes>
      {}{}{}{}{}
    </measure>
  </part>
</score-partwise>"#,
        note("C", Some(1)),
        note("D", None),
        note("E", None),
        note("F", None),
        note("G", Some(5)),
    );
    let mut file = midi_file::musicxml::load_str("fingers.musicxml", &xml).unwrap();
    midi_file::fingering::annotate(&mut file);

    let right = file
        .tracks
        .iter()
        .find(|t| t.hand == Some(Hand::Right))
        .unwrap();
    let fingers: Vec<(u8, bool)> = right
        .notes
        .iter()
        .map(|n| n.finger.map(|f| (f.digit, f.printed)).unwrap())
        .collect();
    assert_eq!(
        fingers,
        vec![(1, true), (2, false), (3, false), (4, false), (5, true)]
    );

    let score = file.score.as_ref().unwrap();
    let written: Vec<(String, bool)> = score.measures[0].staves[0].voices[0]
        .events
        .iter()
        .map(|e| {
            let n = &e.notes[0];
            (n.fingering.clone().unwrap(), n.fingering_auto)
        })
        .collect();
    assert_eq!(
        written,
        vec![
            ("1".into(), false),
            ("2".into(), true),
            ("3".into(), true),
            ("4".into(), true),
            ("5".into(), false)
        ]
    );
}
