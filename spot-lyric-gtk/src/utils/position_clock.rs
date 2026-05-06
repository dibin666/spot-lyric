//! Client-side playback position interpolation.
//!
//! The daemon pushes `PlaybackState` only when something materially changes
//! (track / play-pause / seek). For smooth lyrics line tracking we need a
//! sub-50 ms estimate of the current position, so we snapshot the latest
//! daemon update and extrapolate against the wall clock.

use std::cell::Cell;
use std::time::Instant;

const MAX_SNAPSHOT_BACKWARD_DRIFT_MS: i64 = 250;

#[derive(Debug, Clone, Copy)]
pub struct PositionClock {
    baseline_position_ms: i64,
    baseline_at: Option<Instant>,
    duration_ms: i64,
    is_playing: bool,
    last_snapshot_position_ms: Option<i64>,
}

impl Default for PositionClock {
    fn default() -> Self {
        Self {
            baseline_position_ms: 0,
            baseline_at: None,
            duration_ms: 0,
            is_playing: false,
            last_snapshot_position_ms: None,
        }
    }
}

impl PositionClock {
    /// Reset to the latest snapshot from the daemon.
    pub fn snapshot(&mut self, position_ms: i64, duration_ms: i64, is_playing: bool) -> i64 {
        let raw_position_ms = position_ms.max(0);
        let duration_ms = duration_ms.max(0);
        let before = self.estimate();
        let position_ms = self.stable_snapshot_position(raw_position_ms, duration_ms, is_playing);
        let ignored_backward_drift = is_playing && self.is_playing && position_ms > raw_position_ms;
        tracing::debug!(
            target: "spot_lyric_gtk::timeline",
            raw_position_ms,
            stable_position_ms = position_ms,
            before_estimate_ms = before,
            duration_ms,
            is_playing,
            ignored_backward_drift,
            "position clock snapshot"
        );
        self.baseline_position_ms = position_ms;
        self.baseline_at = Some(Instant::now());
        self.duration_ms = duration_ms;
        self.is_playing = is_playing;
        self.last_snapshot_position_ms = Some(raw_position_ms);
        self.estimate()
    }

    fn stable_snapshot_position(
        &self,
        position_ms: i64,
        duration_ms: i64,
        is_playing: bool,
    ) -> i64 {
        let current = self.estimate();
        let continuous_playback = self.is_playing && is_playing;
        let backward_snapshot = current > position_ms;
        let stale_backward_snapshot = continuous_playback
            && backward_snapshot
            && self.is_stale_backward_snapshot(current, position_ms);
        let stable = if stale_backward_snapshot {
            current
        } else {
            position_ms
        };
        if duration_ms > 0 {
            stable.min(duration_ms)
        } else {
            stable
        }
    }

    fn is_stale_backward_snapshot(&self, current_ms: i64, snapshot_ms: i64) -> bool {
        let Some(previous_snapshot_ms) = self.last_snapshot_position_ms else {
            return current_ms - snapshot_ms <= MAX_SNAPSHOT_BACKWARD_DRIFT_MS;
        };
        if previous_snapshot_ms - snapshot_ms > MAX_SNAPSHOT_BACKWARD_DRIFT_MS {
            return false;
        }
        if snapshot_ms <= previous_snapshot_ms {
            return true;
        }
        current_ms - snapshot_ms <= MAX_SNAPSHOT_BACKWARD_DRIFT_MS
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

    pub fn snapshot(&self, position_ms: i64, duration_ms: i64, is_playing: bool) -> i64 {
        let mut clock = self.0.get();
        let position = clock.snapshot(position_ms, duration_ms, is_playing);
        self.0.set(clock);
        position
    }

    pub fn estimate(&self) -> i64 {
        self.0.get().estimate()
    }

    pub fn reset(&self) {
        self.0.set(PositionClock::default());
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

    #[test]
    fn playing_snapshot_ignores_small_backward_drift() {
        let mut clock = PositionClock::default();
        clock.snapshot(10_000, 60_000, true);
        std::thread::sleep(std::time::Duration::from_millis(80));
        let before = clock.estimate();

        clock.snapshot(9_950, 60_000, true);
        let after = clock.estimate();

        assert!(
            after >= before - 5,
            "small stale backward snapshots should not rewind playback, before={before}, after={after}"
        );
    }

    #[test]
    fn playing_snapshot_accepts_large_backward_seek() {
        let mut clock = PositionClock::default();
        clock.snapshot(30_000, 60_000, true);
        std::thread::sleep(std::time::Duration::from_millis(20));

        clock.snapshot(10_000, 60_000, true);
        let estimate = clock.estimate();

        assert!(
            (10_000..=10_080).contains(&estimate),
            "large backward jumps are user seeks and must be accepted, got {estimate}"
        );
    }

    #[test]
    fn playing_snapshot_ignores_repeated_stale_backward_position() {
        let mut clock = PositionClock::default();
        clock.snapshot(3_000, 60_000, true);
        clock.baseline_at = Some(Instant::now() - std::time::Duration::from_millis(2_100));
        let before = clock.estimate();

        clock.snapshot(3_000, 60_000, true);
        let after = clock.estimate();

        assert!(
            after >= before - 5,
            "repeated stale snapshots should not rewind playback, before={before}, after={after}"
        );
    }

    #[test]
    fn playing_snapshot_accepts_spotify_backward_correction_over_drift_limit() {
        let mut clock = PositionClock::default();
        clock.snapshot(10_000, 60_000, true);
        clock.baseline_at = Some(Instant::now() - std::time::Duration::from_millis(520));

        clock.snapshot(10_200, 60_000, true);
        let estimate = clock.estimate();

        assert!(
            (10_200..=10_260).contains(&estimate),
            "Spotify corrections beyond the small jitter window should be accepted, got {estimate}"
        );
    }
}
