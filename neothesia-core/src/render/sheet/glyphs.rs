//! SMuFL code points (Leland font) and glyph widths.

pub const G_CLEF: char = '\u{E050}';
pub const F_CLEF: char = '\u{E062}';
pub const BRACE: char = '\u{E000}';

pub const NOTEHEAD_WHOLE: char = '\u{E0A2}';
pub const NOTEHEAD_HALF: char = '\u{E0A3}';
pub const NOTEHEAD_BLACK: char = '\u{E0A4}';

pub const SHARP: char = '\u{E262}';
pub const FLAT: char = '\u{E260}';
pub const NATURAL: char = '\u{E261}';

pub const DOT: char = '\u{E1E7}';
pub const STACCATO_ABOVE: char = '\u{E4A2}';
pub const STACCATO_BELOW: char = '\u{E4A3}';

pub const REST_WHOLE: char = '\u{E4E3}';
pub const REST_HALF: char = '\u{E4E4}';
pub const REST_QUARTER: char = '\u{E4E5}';
pub const REST_8TH: char = '\u{E4E6}';
pub const REST_16TH: char = '\u{E4E7}';
pub const REST_32ND: char = '\u{E4E8}';

pub const FLAG_UP: [char; 3] = ['\u{E240}', '\u{E242}', '\u{E244}'];
pub const FLAG_DOWN: [char; 3] = ['\u{E241}', '\u{E243}', '\u{E245}'];

pub const OTTAVA_ALTA: char = '\u{E511}';
pub const OTTAVA_BASSA: char = '\u{E51C}';
pub const TRILL: char = '\u{E566}';
/// Small note with a slashed stem
pub const GRACE_ACCIACCATURA: char = '\u{E560}';

pub fn time_sig_digit(d: u32) -> char {
    char::from_u32(0xE080 + d.min(9)).unwrap()
}

pub fn tuplet_digit(d: u32) -> char {
    char::from_u32(0xE880 + d.min(9)).unwrap()
}

/// Glyph widths in staff spaces
#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    pub head_black: f32,
    pub head_half: f32,
    pub head_whole: f32,
    pub sharp: f32,
    pub flat: f32,
    pub natural: f32,
    pub dot: f32,
    pub flag: f32,
    pub time_digit: f32,
    pub clef: f32,
}

impl Default for Metrics {
    /// Approximate widths, close to Leland's metadata
    fn default() -> Self {
        Self {
            head_black: 1.18,
            head_half: 1.18,
            head_whole: 1.68,
            sharp: 1.0,
            flat: 0.9,
            natural: 0.68,
            dot: 0.4,
            flag: 1.05,
            time_digit: 1.8,
            clef: 2.7,
        }
    }
}

impl Metrics {
    pub fn accidental(&self, c: char) -> f32 {
        match c {
            SHARP => self.sharp,
            FLAT => self.flat,
            _ => self.natural,
        }
    }
}
