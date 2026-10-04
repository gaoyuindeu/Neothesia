//! A–B repeat: mark where a section starts (A) and ends (B), then only that section plays,
//! over and over. The marks are kept per song in the settings.

use std::{path::PathBuf, time::Duration};

use neothesia_core::config::SavedLoop;

use crate::{context::Context, song::Song};

/// Marks sit this far before a note: the player skips the events at the time it is set to,
/// and the note should sound when the section starts again
const MARGIN: Duration = Duration::from_millis(30);
/// How long after a note is heard the A key comes, in real time
const REACTION: Duration = Duration::from_millis(250);
/// Shortest section
const MIN_LENGTH: Duration = Duration::from_millis(100);

pub struct Looper {
    song: Option<PathBuf>,
    lead_in: Duration,
    /// Player times (with the lead-in)
    start: Option<Duration>,
    end: Option<Duration>,
    active: bool,
}

impl Looper {
    /// The marks saved for the open song
    pub fn new(ctx: &Context, lead_in: Duration) -> Self {
        let song = ctx.config.last_opened_song().cloned();
        let saved = song.as_deref().and_then(|s| ctx.config.song_loop(s));
        let time = |secs: f32| lead_in + Duration::from_secs_f32(secs.max(0.0));
        Self {
            start: saved.map(|l| time(l.start)),
            end: saved.and_then(|l| l.end).map(time),
            active: saved.is_some_and(|l| l.active && l.end.is_some()),
            song,
            lead_in,
        }
    }

    pub fn start(&self) -> Option<Duration> {
        self.start
    }

    pub fn end(&self) -> Option<Duration> {
        self.end
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Where to jump when playback at `time` has passed the end of the section
    pub fn wrap(&self, time: Duration) -> Option<Duration> {
        match (self.active, self.start, self.end) {
            (true, Some(start), Some(end)) if time > end => Some(start),
            _ => None,
        }
    }

    /// Mark A. A later B than A is kept; one before it is dropped.
    pub fn set_start(&mut self, time: Duration) {
        self.start = Some(time);
        if self.end.is_some_and(|end| end < time + MIN_LENGTH) {
            self.end = None;
            self.active = false;
        }
    }

    /// Mark B and start repeating; the start of the section to jump to, or None when the
    /// section would be empty. Without A the section starts at the beginning; B before A
    /// swaps them.
    pub fn set_end(&mut self, time: Duration) -> Option<Duration> {
        let start = self.start.unwrap_or(Duration::ZERO);
        let (start, end) = if time < start {
            (time, start)
        } else {
            (start, time)
        };
        if end < start + MIN_LENGTH {
            return None;
        }
        self.start = Some(start);
        self.end = Some(end);
        self.active = true;
        Some(start)
    }

    /// Turn repeating on or off. Turning it on without B makes a section of a few seconds
    /// from A (or from `now`).
    pub fn toggle(&mut self, now: Duration, length: Duration) {
        if self.active {
            self.active = false;
            return;
        }
        let start = self.start.unwrap_or(now);
        let end = self
            .end
            .filter(|&end| end >= start + MIN_LENGTH)
            .unwrap_or_else(|| (start + Duration::from_secs(5)).min(length));
        if end >= start + MIN_LENGTH {
            self.start = Some(start);
            self.end = Some(end);
            self.active = true;
        }
    }

    pub fn clear(&mut self) {
        self.start = None;
        self.end = None;
        self.active = false;
    }

    /// Drag A (it stays before B)
    pub fn drag_start(&mut self, time: Duration) {
        match self.end {
            Some(end) => self.start = Some(time.min(end.saturating_sub(MIN_LENGTH))),
            None => self.start = Some(time),
        }
    }

    /// Drag B (it stays after A)
    pub fn drag_end(&mut self, time: Duration) {
        let start = self.start.unwrap_or(Duration::ZERO);
        self.end = Some(time.max(start + MIN_LENGTH));
    }

    pub fn save(&self, ctx: &mut Context) {
        let Some(song) = &self.song else {
            return;
        };
        let secs = |time: Duration| time.saturating_sub(self.lead_in).as_secs_f32();
        let saved = self.start.map(|start| SavedLoop {
            song: song.clone(),
            start: secs(start),
            end: self.end.map(secs),
            active: self.active,
        });
        if ctx.config.song_loop(song) != saved.as_ref() {
            ctx.config.set_song_loop(song, saved);
        }
    }
}

/// Where A goes when it is pressed at `time` (player time) at playback `speed`: back to the
/// note that was just heard when one began a moment before (the key comes after the ear),
/// and a little before that note so it is played again when the section repeats
pub fn snap_start(
    song: &Song,
    lead_in: Duration,
    time: Duration,
    speed: f32,
    playing: bool,
) -> Duration {
    let song_time = time.saturating_sub(lead_in);
    let window = if playing {
        REACTION.mul_f32(speed.max(0.05))
    } else {
        MARGIN
    };
    let onset = song
        .file
        .tracks
        .iter()
        .flat_map(|track| track.notes.iter())
        .map(|note| note.start)
        .filter(|&start| start <= song_time && song_time - start <= window)
        .max();
    match onset {
        Some(onset) => (onset + lead_in).saturating_sub(MARGIN),
        None => time,
    }
}

/// "1:05.3"
pub fn format_time(time: Duration, lead_in: Duration) -> String {
    let secs = time.as_secs_f32() - lead_in.as_secs_f32();
    let sign = if secs < 0.0 { "-" } else { "" };
    let secs = secs.abs();
    let minutes = (secs / 60.0).floor();
    format!("{sign}{}:{:04.1}", minutes as u32, secs - minutes * 60.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn looper() -> Looper {
        Looper {
            song: None,
            lead_in: Duration::from_secs(3),
            start: None,
            end: None,
            active: false,
        }
    }

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    #[test]
    fn a_then_b_repeats_the_section() {
        let mut l = looper();
        l.set_start(secs(10.0));
        assert!(!l.is_active());
        assert_eq!(l.set_end(secs(14.0)), Some(secs(10.0)));
        assert!(l.is_active());
        assert_eq!(l.wrap(secs(13.0)), None);
        assert_eq!(l.wrap(secs(14.1)), Some(secs(10.0)));
    }

    #[test]
    fn b_before_a_swaps() {
        let mut l = looper();
        l.set_start(secs(10.0));
        assert_eq!(l.set_end(secs(6.0)), Some(secs(6.0)));
        assert_eq!((l.start(), l.end()), (Some(secs(6.0)), Some(secs(10.0))));
    }

    #[test]
    fn a_after_b_drops_b() {
        let mut l = looper();
        l.set_start(secs(10.0));
        l.set_end(secs(14.0));
        l.set_start(secs(12.0));
        assert!(l.is_active(), "B is still after A");
        l.set_start(secs(20.0));
        assert_eq!(l.end(), None);
        assert!(!l.is_active());
    }

    #[test]
    fn toggle_keeps_the_marks() {
        let mut l = looper();
        l.set_start(secs(10.0));
        l.set_end(secs(14.0));
        l.toggle(secs(11.0), secs(100.0));
        assert!(!l.is_active());
        l.toggle(secs(50.0), secs(100.0));
        assert!(l.is_active());
        assert_eq!((l.start(), l.end()), (Some(secs(10.0)), Some(secs(14.0))));
    }

    #[test]
    fn time_format() {
        let lead_in = secs(3.0);
        assert_eq!(format_time(secs(68.3), lead_in), "1:05.3");
        assert_eq!(format_time(secs(3.0), lead_in), "0:00.0");
    }
}
