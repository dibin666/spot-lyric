//! Client-side playback position interpolation.
//!
//! The daemon pushes `PlaybackState` only when something materially changes
//! (track / play-pause / seek). For smooth lyrics line tracking we need a
//! sub-50 ms estimate of the current position, so we snapshot the latest
//! daemon update and extrapolate against the wall clock.

use std::cell::Cell;
use std::time::Instant;

#[derive(Debug, Clone, Copy)]
pub struct PositionClock {
    baseline_position_ms: i64,
    baseline_at: Option<Instant>,
    duration_ms: i64,
    is_playing: bool,
}

impl Default for PositionClock {
    fn default() -> Self {
        Self {
            baseline_position_ms: 0,
            baseline_at: None,
            duration_ms: 0,
            is_playing: false,
        }
    }
}

impl PositionClock {
    /// Reset to the latest snapshot from the daemon.
    pub fn snapshot(&mut self, position_ms: i64, duration_ms: i64, is_playing: bool) {
        self.baseline_position_ms = position_ms.max(0);
        self.baseline_at = Some(Instant::now());
        self.duration_ms = duration_ms.max(0);
        self.is_playing = is_playing;
    }

    pub fn is_playing(&self) -> bool {
        self.is_playing
    }

    /// Current best estimate of position in ms, clamped to `[0, duration]`.
    pub fn estimate(&self) -> i64 {
        let baseline = match self.baseline_at {
            Some(at) => at,
            None => return 0,
        };
        let mut position = self.baseline_position_ms;
        if self.is_playing {
            position += baseline.elapsed().as_millis() as i64;
        }
        if self.duration_ms > 0 {
            position = position.min(self.duration_ms);
        }
        position.max(0)
    }
}

/// Cell-friendly wrapper for use inside `RefCell`-free structs.
pub struct CellClock(Cell<PositionClock>);

impl CellClock {
    pub fn new() -> Self {
        Self(Cell::new(PositionClock::default()))
    }

    pub fn snapshot(&self, position_ms: i64, duration_ms: i64, is_playing: bool) {
        let mut clock = self.0.get();
        clock.snapshot(position_ms, duration_ms, is_playing);
        self.0.set(clock);
    }

    pub fn estimate(&self) -> i64 {
        self.0.get().estimate()
    }
}

impl Default for CellClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_returns_baseline_when_paused() {
        let mut clock = PositionClock::default();
        clock.snapshot(12_000, 30_000, false);
        std::thread::sleep(std::time::Duration::from_millis(80));
        let estimate = clock.estimate();
        assert!(
            (12_000..=12_010).contains(&estimate),
            "paused clock should not advance, got {estimate}"
        );
    }

    #[test]
    fn estimate_clamps_to_duration() {
        let mut clock = PositionClock::default();
        clock.snapshot(29_990, 30_000, true);
        std::thread::sleep(std::time::Duration::from_millis(40));
        assert!(clock.estimate() <= 30_000);
    }

    #[test]
    fn estimate_is_zero_until_first_snapshot() {
        let clock = PositionClock::default();
        assert_eq!(clock.estimate(), 0);
    }
}
