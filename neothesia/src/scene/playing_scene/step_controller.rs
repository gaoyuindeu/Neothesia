//! Step mode: instead of playing continuously, `→` advances to the next note (or chord)
//! and `←` goes back to the previous one.

use std::time::Duration;

use winit::{
    event::WindowEvent,
    keyboard::{Key, NamedKey},
};

use super::MidiPlayer;
use crate::{
    song::{PlayerConfig, Song},
    utils::window::WinitEvent,
};

/// Notes starting within this window form one step
const CHORD_WINDOW: Duration = Duration::from_millis(30);
/// How long the waterfall takes to glide to the next step
const GLIDE: Duration = Duration::from_millis(120);
/// Tolerance when comparing the player time with step onsets
const EPSILON: Duration = Duration::from_millis(1);

pub struct StepController {
    enabled: bool,
    /// Player timestamps (lead-in included) of every step, sorted
    onsets: Vec<Duration>,
    glide: Option<Glide>,
}

struct Glide {
    target: Duration,
    /// Player time advanced per second of real time
    speed: f64,
}

impl StepController {
    pub fn new(song: &Song, lead_in: Duration) -> Self {
        let mut starts: Vec<Duration> = song
            .file
            .tracks
            .iter()
            .filter(|track| {
                let config = &song.config.tracks[track.track_id];
                config.visible && config.player != PlayerConfig::Mute
            })
            .flat_map(|track| track.notes.iter())
            .filter(|note| note.channel != 9)
            .map(|note| note.start + lead_in)
            .collect();

        Self {
            enabled: false,
            onsets: group_onsets(starts),
            glide: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn toggle(&mut self, player: &mut MidiPlayer) {
        self.set_enabled(!self.enabled, player);
    }

    pub fn set_enabled(&mut self, enabled: bool, player: &mut MidiPlayer) {
        self.enabled = enabled;
        self.glide = None;
        if enabled {
            player.pause();
        }
    }

    /// Returns true if the event was consumed by step mode
    pub fn handle_window_event(&mut self, event: &WindowEvent, player: &mut MidiPlayer) -> bool {
        if !self.enabled {
            return false;
        }

        let right = Key::Named(NamedKey::ArrowRight);
        let left = Key::Named(NamedKey::ArrowLeft);

        if event.key_pressed(right.clone()) {
            self.step_forward(player);
            return true;
        }
        if event.key_pressed(left.clone()) {
            self.step_back(player);
            return true;
        }

        // Swallow the rest of the arrow key events so that they don't start rewinding
        event.key_released(right) || event.key_released(left)
    }

    fn current_time(&self, player: &MidiPlayer) -> Duration {
        // While gliding, act as if we already arrived
        self.glide
            .as_ref()
            .map_or(player.time(), |glide| glide.target)
    }

    fn step_forward(&mut self, player: &mut MidiPlayer) {
        // Pressing again mid-glide retargets the glide, so fast presses don't lag behind
        let now = self.current_time(player);

        let Some(target) = next_onset(&self.onsets, now) else {
            return;
        };
        self.glide_to(player, target);
    }

    fn step_back(&mut self, player: &mut MidiPlayer) {
        let now = self.current_time(player);
        self.glide = None;

        let Some(target) = prev_onset(&self.onsets, now) else {
            return;
        };

        // Jump just before the step, then glide onto it so that its notes sound
        let before = target.saturating_sub(GLIDE.min(Duration::from_millis(50)));
        player.set_time(before);
        self.glide_to(player, target);
    }

    fn glide_to(&mut self, player: &MidiPlayer, target: Duration) {
        let distance = target.saturating_sub(player.time());
        self.glide = Some(Glide {
            target,
            speed: distance.as_secs_f64() / GLIDE.as_secs_f64(),
        });
    }

    /// How far the player should advance this frame
    pub fn update(&mut self, player: &MidiPlayer, delta: Duration) -> Duration {
        let Some(glide) = &self.glide else {
            return Duration::ZERO;
        };

        let remaining = glide.target.saturating_sub(player.time());
        let advance = Duration::from_secs_f64(delta.as_secs_f64() * glide.speed).min(remaining);

        if advance >= remaining {
            self.glide = None;
        }

        advance
    }

    /// Called when time was changed from outside (mouse seek, looper)
    pub fn cancel_glide(&mut self) {
        self.glide = None;
    }
}

/// Sort note starts and merge the ones closer than `CHORD_WINDOW` into one step
fn group_onsets(mut starts: Vec<Duration>) -> Vec<Duration> {
    starts.sort();

    let mut onsets: Vec<Duration> = Vec::new();
    for start in starts {
        match onsets.last() {
            Some(&last) if start <= last + CHORD_WINDOW => {}
            _ => onsets.push(start),
        }
    }
    onsets
}

fn next_onset(onsets: &[Duration], now: Duration) -> Option<Duration> {
    onsets.iter().copied().find(|&t| t > now + EPSILON)
}

fn prev_onset(onsets: &[Duration], now: Duration) -> Option<Duration> {
    onsets.iter().rev().copied().find(|&t| t + EPSILON < now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn chords_are_grouped() {
        let onsets = group_onsets(vec![ms(500), ms(0), ms(10), ms(25), ms(520), ms(1000)]);
        assert_eq!(onsets, vec![ms(0), ms(500), ms(1000)]);
    }

    #[test]
    fn navigation() {
        let onsets = vec![ms(0), ms(500), ms(1000)];

        // Standing on a step: next is the following one, previous the one before
        assert_eq!(next_onset(&onsets, ms(500)), Some(ms(1000)));
        assert_eq!(prev_onset(&onsets, ms(500)), Some(ms(0)));

        // Between steps
        assert_eq!(next_onset(&onsets, ms(700)), Some(ms(1000)));
        assert_eq!(prev_onset(&onsets, ms(700)), Some(ms(500)));

        // Ends
        assert_eq!(next_onset(&onsets, ms(1000)), None);
        assert_eq!(prev_onset(&onsets, ms(0)), None);
    }
}
