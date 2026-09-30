use midi_file::{
    Hand,
    musicxml::load_str,
    score::{Accidental, Clef, DirectionKind, NoteValue},
};

/// Two staves, 4/4, divisions 2 (eighths)
fn score(measures: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<score-partwise version="3.1">
  <part-list><score-part id="P1"><part-name>Piano</part-name></score-part></part-list>
  <part id="P1">{measures}</part>
</score-partwise>"#
    )
}

const ATTRIBUTES: &str = r#"<attributes>
  <divisions>2</divisions>
  <key><fifths>-1</fifths></key>
  <time><beats>4</beats><beat-type>4</beat-type></time>
  <staves>2</staves>
  <clef number="1"><sign>G</sign><line>2</line></clef>
  <clef number="2"><sign>F</sign><line>4</line></clef>
</attributes>
<direction placement="below"><direction-type><dynamics><p/></dynamics></direction-type><staff>1</staff></direction>
<direction placement="above"><direction-type><words font-style="italic">dolce</words></direction-type><staff>1</staff><sound tempo="60"/></direction>"#;

fn note(
    step: &str,
    octave: i32,
    duration: u32,
    kind: &str,
    staff: u32,
    voice: u32,
    extra: &str,
) -> String {
    format!(
        "<note><pitch><step>{step}</step><octave>{octave}</octave></pitch><duration>{duration}</duration><voice>{voice}</voice><type>{kind}</type><staff>{staff}</staff>{extra}</note>"
    )
}

#[test]
fn notes_voices_and_marks() {
    let m1 = format!(
        r#"<measure number="1">{ATTRIBUTES}
        {}{}{}
        {}
        <backup><duration>8</duration></backup>
        {}
        </measure>"#,
        // Treble: C5 quarter (slur start, staccato), D5 quarter (slur stop), E5 half tied
        note(
            "C",
            5,
            2,
            "quarter",
            1,
            1,
            r#"<notations><slur type="start" number="1"/><articulations><staccato/></articulations></notations>"#
        ),
        note(
            "D",
            5,
            2,
            "quarter",
            1,
            1,
            r#"<notations><slur type="stop" number="1"/></notations>"#
        ),
        note(
            "E",
            5,
            4,
            "half",
            1,
            1,
            r#"<tie type="start"/><notations><tied type="start"/></notations>"#
        ),
        // Chord tone with the E5
        "",
        // Bass: F3 whole
        note("F", 3, 8, "whole", 2, 5, ""),
    );
    let m2 = format!(
        r#"<measure number="2">
        {}
        <note><grace slash="yes"/><pitch><step>B</step><alter>-1</alter><octave>4</octave></pitch><voice>1</voice><type>eighth</type><staff>1</staff><accidental>flat</accidental></note>
        {}{}{}
        <backup><duration>8</duration></backup>
        {}
        <attributes><clef number="2"><sign>G</sign><line>2</line></clef></attributes>
        {}
        </measure>"#,
        note(
            "E",
            5,
            2,
            "quarter",
            1,
            1,
            r#"<tie type="stop"/><notations><tied type="stop"/></notations>"#
        ),
        // Triplet eighths in the time of a quarter (duration 4/3 can't be expressed with
        // divisions 2, so use a quarter-note triplet: 3 quarters in 2 beats -> 4/3... keep simple)
        note("A", 4, 2, "quarter", 1, 1, ""),
        note("G", 4, 2, "quarter", 1, 1, ""),
        note("F", 4, 2, "quarter", 1, 1, ""),
        note("C", 4, 4, "half", 2, 5, ""),
        note("C", 4, 4, "half", 2, 5, ""),
    );
    let xml = score(&format!("{m1}{m2}"));
    let file = load_str("test.musicxml", &xml).unwrap();
    let score = file.score.as_ref().unwrap();

    assert_eq!(score.measures.len(), 2);
    assert_eq!(score.measures[0].key, -1);

    // Playback: tie merged, hands from the staves, tempo 60 (a quarter lasts a second)
    let right = file
        .tracks
        .iter()
        .find(|t| t.hand == Some(Hand::Right))
        .unwrap();
    let left = file
        .tracks
        .iter()
        .find(|t| t.hand == Some(Hand::Left))
        .unwrap();
    let right_pitches: Vec<u8> = right.notes.iter().map(|n| n.note).collect();
    assert!(right_pitches.contains(&76));
    assert_eq!(
        right_pitches.iter().filter(|&&p| p == 76).count(),
        1,
        "tied E5 plays once"
    );
    assert_eq!(left.notes.len(), 3);
    let e5 = right.notes.iter().find(|n| n.note == 76).unwrap();
    assert!((e5.start.as_secs_f64() - 2.0).abs() < 0.01);
    assert!((e5.duration.as_secs_f64() - 3.0).abs() < 0.05);

    // Notation
    let treble = &score.measures[0].staves[0].voices[0];
    let values: Vec<NoteValue> = treble.events.iter().map(|e| e.value).collect();
    assert_eq!(
        values,
        vec![NoteValue::Quarter, NoteValue::Quarter, NoteValue::Half]
    );
    assert!(treble.events[0].staccato);
    assert!(treble.events[2].notes[0].tie_to_next);
    assert_eq!(score.slurs.len(), 1);
    assert_eq!(score.slurs[0].start.event, 0);
    assert_eq!(score.slurs[0].end.event, 1);

    let directions = &score.measures[0].directions;
    assert!(
        directions
            .iter()
            .any(|d| d.kind == DirectionKind::Dynamic("p".into()))
    );
    assert!(directions.iter().any(
        |d| matches!(&d.kind, DirectionKind::Words { text, italic: true, .. } if text == "dolce")
    ));

    // Grace note hangs on the next chord, with its spelled accidental
    let second = &score.measures[1].staves[0].voices[0];
    let with_grace = second.events.iter().find(|e| !e.grace.is_empty()).unwrap();
    assert_eq!(with_grace.grace[0].accidental, Some(Accidental::Flat));

    // Clef change in the bass staff
    assert_eq!(score.measures[1].staves[1].clef, Clef::Bass);
    assert_eq!(score.measures[1].staves[1].clef_changes.len(), 1);
    let bass = &score.measures[1].staves[1].voices[0].events;
    assert_eq!((bass[0].clef, bass[1].clef), (Clef::Bass, Clef::Treble));
}

#[test]
fn repeats_and_voltas_unroll() {
    // | 1 :| with volta 1 on measure 2, volta 2 on measure 3
    let quarter_notes = |step: &str| {
        (0..4)
            .map(|_| note(step, 5, 2, "quarter", 1, 1, ""))
            .collect::<String>()
    };
    let xml = score(&format!(
        r#"<measure number="1"><attributes><divisions>2</divisions><time><beats>4</beats><beat-type>4</beat-type></time><clef><sign>G</sign><line>2</line></clef></attributes>
            <barline location="left"><repeat direction="forward"/></barline>{}</measure>
           <measure number="2"><barline location="left"><ending number="1" type="start"/></barline>{}
            <barline location="right"><ending number="1" type="stop"/><repeat direction="backward"/></barline></measure>
           <measure number="3"><barline location="left"><ending number="2" type="start"/></barline>{}
            <barline location="right"><ending number="2" type="discontinue"/></barline></measure>"#,
        quarter_notes("C"),
        quarter_notes("D"),
        quarter_notes("E"),
    ));
    let file = load_str("repeat.musicxml", &xml).unwrap();
    let score = file.score.as_ref().unwrap();
    let numbers: Vec<&str> = score.measures.iter().map(|m| m.number.as_str()).collect();
    assert_eq!(numbers, vec!["1", "2", "1", "3"]);
    assert_eq!(score.measures[1].ending.as_deref(), Some("1."));
    assert!(score.measures[1].repeat_end);
}

#[test]
fn pickup_measure_keeps_its_length() {
    let xml = score(&format!(
        r#"<measure number="0" implicit="yes"><attributes><divisions>2</divisions><time><beats>3</beats><beat-type>4</beat-type></time><clef><sign>G</sign><line>2</line></clef></attributes>{}</measure>
           <measure number="1">{}{}{}</measure>"#,
        note("G", 4, 2, "quarter", 1, 1, ""),
        note("C", 5, 2, "quarter", 1, 1, ""),
        note("C", 5, 2, "quarter", 1, 1, ""),
        note("C", 5, 2, "quarter", 1, 1, ""),
    ));
    let file = load_str("pickup.musicxml", &xml).unwrap();
    let score = file.score.as_ref().unwrap();
    let ppq = score.ppq as u64;
    assert_eq!(
        score.measures[0].end_tick - score.measures[0].start_tick,
        ppq
    );
    assert_eq!(score.measures[1].start_tick, ppq);
}
