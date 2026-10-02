//! Merge all tracks of a MIDI file into one (format 0), e.g. for a piano piece written as
//! one track per voice, so the hand split works on it: `merge_tracks <in.mid> <out.mid>`
use midly::{Format, MetaMessage, Smf, TrackEvent, TrackEventKind};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: merge_tracks <in.mid> <out.mid>");
        std::process::exit(2);
    }
    let bytes = std::fs::read(&args[0]).expect("read");
    let smf = Smf::parse(&bytes).expect("parse");

    // (absolute tick, track, order in track, event); end of track markers are dropped
    let mut events: Vec<(u64, usize, usize, TrackEventKind)> = Vec::new();
    for (t, track) in smf.tracks.iter().enumerate() {
        let mut tick = 0u64;
        for (i, e) in track.iter().enumerate() {
            tick += e.delta.as_int() as u64;
            if !matches!(e.kind, TrackEventKind::Meta(MetaMessage::EndOfTrack)) {
                events.push((tick, t, i, e.kind));
            }
        }
    }
    events.sort_by_key(|(tick, t, i, _)| (*tick, *t, *i));

    let mut out: Vec<TrackEvent> = Vec::with_capacity(events.len() + 1);
    let mut last = 0u64;
    for (tick, _, _, kind) in events {
        out.push(TrackEvent {
            delta: ((tick - last) as u32).into(),
            kind,
        });
        last = tick;
    }
    out.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });

    let merged = Smf {
        header: midly::Header::new(Format::SingleTrack, smf.header.timing),
        tracks: vec![out],
    };
    merged.save(&args[1]).expect("write");
    println!("{} tracks merged into one", smf.tracks.len());
}
