//! Time-or-count checkpoint trigger for the apply loop. Checkpointing
//! (itdb_write + manifest save) is expensive/seeky on a spinning-HDD iPod, so
//! we bound BOTH the number of tracks and the wall-clock since the last flush:
//! whichever comes first fires a checkpoint. The time bound caps abrupt-unplug
//! loss to ~`max_interval` regardless of how slow individual (hi-res) tracks are.

use std::time::{Duration, Instant};

pub struct CheckpointClock {
    tracks_since: usize,
    last: Instant,
    max_tracks: usize,
    max_interval: Duration,
}

impl CheckpointClock {
    pub fn new(max_tracks: usize, max_interval: Duration, now: Instant) -> Self {
        Self {
            tracks_since: 0,
            last: now,
            max_tracks,
            max_interval,
        }
    }

    /// Record one committed track. Returns `true` if a checkpoint is due now
    /// (and resets the counters). `now` is injected for testability.
    pub fn record(&mut self, now: Instant) -> bool {
        self.tracks_since += 1;
        let due = self.tracks_since >= self.max_tracks
            || now.duration_since(self.last) >= self.max_interval;
        if due {
            self.tracks_since = 0;
            self.last = now;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::CheckpointClock;
    use std::time::{Duration, Instant};

    #[test]
    fn fires_on_count_bound() {
        let t0 = Instant::now();
        // Large interval so only the count bound can fire.
        let mut c = CheckpointClock::new(3, Duration::from_secs(3600), t0);
        assert!(!c.record(t0));
        assert!(!c.record(t0));
        assert!(c.record(t0)); // 3rd track
        assert!(!c.record(t0)); // reset → counting again
    }

    #[test]
    fn fires_on_time_bound_independent_of_count() {
        let t0 = Instant::now();
        // max_tracks huge so only the time bound can fire; zero interval => the
        // first record already satisfies `elapsed >= 0`.
        let mut c = CheckpointClock::new(10_000, Duration::ZERO, t0);
        assert!(c.record(t0));
    }
}
