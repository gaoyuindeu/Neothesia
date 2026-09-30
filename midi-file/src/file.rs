use crate::{MidiTrack, program_track::ProgramTrack, tempo_track::TempoTrack};
use midly::{Format, MetaMessage, Smf, Timing, TrackEventKind};
use std::{fs, path::Path, sync::Arc};

#[derive(Debug, Clone)]
pub struct MidiFile {
    pub name: String,
    pub format: Format,
    pub tracks: Arc<[MidiTrack]>,
    pub program_track: ProgramTrack,
    pub tempo_track: TempoTrack,
    pub measures: Arc<[std::time::Duration]>,

    /// Ticks (pulses) per quarter note
    pub ppq: u16,
    pub time_signatures: Arc<[TimeSignature]>,
    pub key_signatures: Arc<[KeySignature]>,

    /// Notation read from a score file (MusicXML); None for plain MIDI
    pub score: Option<Arc<crate::score::Score>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSignature {
    pub tick: u64,
    pub numerator: u8,
    pub denominator: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySignature {
    pub tick: u64,
    /// Positive: number of sharps, negative: number of flats
    pub sharps: i8,
    pub minor: bool,
}

/// Collect time and key signature changes from all tracks, sorted by tick
fn signatures(smf: &Smf<'_>) -> (Vec<TimeSignature>, Vec<KeySignature>) {
    let mut time = Vec::new();
    let mut key = Vec::new();

    for track in smf.tracks.iter() {
        let mut tick = 0u64;
        for event in track {
            tick += event.delta.as_int() as u64;
            match event.kind {
                TrackEventKind::Meta(MetaMessage::TimeSignature(num, denom_pow, _, _)) => {
                    time.push(TimeSignature {
                        tick,
                        numerator: num,
                        denominator: 1u8.checked_shl(denom_pow as u32).unwrap_or(4),
                    });
                }
                TrackEventKind::Meta(MetaMessage::KeySignature(sharps, minor)) => {
                    key.push(KeySignature {
                        tick,
                        sharps,
                        minor,
                    });
                }
                _ => {}
            }
        }
    }

    time.sort_by_key(|t| t.tick);
    time.dedup_by_key(|t| t.tick);
    key.sort_by_key(|k| k.tick);
    key.dedup_by_key(|k| k.tick);
    (time, key)
}

impl MidiFile {
    /// Read a MIDI or MusicXML file; notes of the two hands get fingers
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let mut file = Self::load(path.as_ref())?;
        crate::fingering::annotate(&mut file);
        Ok(file)
    }

    fn load(path: &Path) -> Result<Self, String> {
        if crate::musicxml::is_musicxml(path.as_ref()) {
            return crate::musicxml::load(path.as_ref());
        }
        let midi = Self::load_midi(path.as_ref())?;

        // A score next to the recording: notation and hands come from it
        if let Some(score_path) = crate::align::find_score_for(path.as_ref())
            && let Ok(score) = crate::musicxml::load(&score_path)
        {
            let (midi, matched) = crate::align::attach_score(midi, &score);
            if let Some(matched) = matched {
                log::info!(
                    "Score {} matched {:.0}% of its notes{}",
                    score_path.display(),
                    matched * 100.0,
                    if midi.score.is_some() {
                        ""
                    } else {
                        ", not used"
                    }
                );
            }
            return Ok(midi);
        }
        Ok(midi)
    }

    /// Only the MIDI file, without looking for a score next to it
    pub fn load_midi(path: &Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .ok_or(String::from("File not found"))?
            .to_string_lossy()
            .to_string();

        let data = match fs::read(path) {
            Ok(buff) => buff,
            Err(_) => return Err(String::from("Could Not Open File")),
        };

        let smf = match Smf::parse(&data) {
            Ok(smf) => smf,
            Err(_) => return Err(String::from("Midi Parsing Error (midly lib)")),
        };

        Self::from_parsed_smf(name, &smf)
    }

    pub fn from_smf(name: impl Into<String>, smf: &Smf<'_>) -> Result<Self, String> {
        Self::from_parsed_smf(name.into(), smf)
    }

    /// Like `from_smf`, but keeps the tracks as they are (no hand splitting)
    pub(crate) fn from_smf_unsplit(name: String, smf: &Smf<'_>) -> Result<Self, String> {
        Self::build(name, smf, false)
    }

    fn from_parsed_smf(name: String, smf: &Smf<'_>) -> Result<Self, String> {
        Self::build(name, smf, true)
    }

    fn build(name: String, smf: &Smf<'_>, split_hands: bool) -> Result<Self, String> {
        let u_per_quarter_note: u16 = match smf.header.timing {
            Timing::Metrical(t) => t.as_int(),
            Timing::Timecode(_fps, _u) => {
                return Err(String::from("Midi With Timecode Timing, Not Supported!"));
            }
        };

        if smf.tracks.is_empty() {
            return Err(String::from("Midi File Has No Tracks"));
        }

        let tempo_track = TempoTrack::build(&smf.tracks, u_per_quarter_note);

        let mut track_color_id = 0;
        let tracks: Vec<MidiTrack> = smf
            .tracks
            .iter()
            .enumerate()
            .map(|(id, events)| {
                let track = MidiTrack::new(id, track_color_id, &tempo_track, events);

                if !track.notes.is_empty() {
                    track_color_id += 1;
                }

                track
            })
            .collect();

        let tracks = if split_hands {
            crate::hands::assign_hands(tracks)
        } else {
            tracks
        };

        let measures = {
            let last_note_end = tracks
                .iter()
                .fold(std::time::Duration::ZERO, |last, track| {
                    if let Some(note) = track.notes.last() {
                        last.max(note.start + note.duration)
                    } else {
                        last
                    }
                });

            let mut masures = Vec::new();
            let mut time = std::time::Duration::ZERO;
            let mut id = 0;
            while time <= last_note_end {
                time = tempo_track.pulses_to_duration(id * u_per_quarter_note as u64 * 4);
                masures.push(time);
                id += 1;
            }

            masures
        };

        let program_track = ProgramTrack::new(&tracks);
        let (time_signatures, key_signatures) = signatures(smf);

        Ok(Self {
            name,
            format: smf.header.format,
            tracks: tracks.into(),
            program_track,
            tempo_track,
            measures: measures.into(),
            ppq: u_per_quarter_note,
            time_signatures: time_signatures.into(),
            key_signatures: key_signatures.into(),
            score: None,
        })
    }
}
