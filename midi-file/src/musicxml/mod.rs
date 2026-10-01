//! MusicXML scores: notation straight from the file (dynamics, slurs, voices, ...)
//! and MIDI generated from it for playback.

mod build;
mod fingering_transfer;
pub mod omr;
mod parse;

use std::path::Path;

use crate::MidiFile;

pub use fingering_transfer::{
    TransferReport, note_fingerings, strip_fingering, transfer as transfer_fingering,
    transfer_into_file as transfer_fingering_into_file,
};
pub use parse::{RawScore, parse, read_file};

/// File extensions of MusicXML scores
pub const EXTENSIONS: [&str; 3] = ["musicxml", "mxl", "xml"];

pub fn is_musicxml(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Load a MusicXML file as a playable song with its notation attached
pub fn load(path: &Path) -> Result<MidiFile, String> {
    let name = path
        .file_name()
        .ok_or("File not found")?
        .to_string_lossy()
        .to_string();
    let xml = read_file(path)?;
    let raw = parse(&xml)?;
    build::build(raw, name)
}

/// Same as `load`, from the XML text
pub fn load_str(name: &str, xml: &str) -> Result<MidiFile, String> {
    build::build(parse(xml)?, name.to_string())
}
