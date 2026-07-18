//! Periodic scheduler. Yields `()` ticks at a configurable interval
//! (in minutes). 0 disables. The daemon runtime is responsible for
//! converting a tick into a `SyncTrigger::Scheduled` via the state
//! machine.

use std::time::Duration;
use tokio::time::{Instant, Interval};

pub struct SyncScheduler {
    interval: Option<Interval>,
    minutes: u32,
}

impl SyncScheduler {
    /// Build a scheduler that fires every `minutes` minutes. 0 disables.
    ///
    /// The first tick fires at +`minutes` minutes from construction (NOT
    /// immediately). Implemented via `interval_at(Instant::now() + period, ...)`
    /// so we don't need to consume a "free" immediate tick at runtime.
    pub fn new(minutes: u32) -> Self {
        let interval = if minutes == 0 {
            None
        } else {
            let period = Duration::from_secs(minutes as u64 * 60);
            let mut i = tokio::time::interval_at(Instant::now() + period, period);
            i.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            Some(i)
        };
        Self { interval, minutes }
    }

    pub fn minutes(&self) -> u32 {
        self.minutes
    }

    pub fn is_disabled(&self) -> bool {
        self.interval.is_none()
    }

    /// Re-arm with a new interval. Call when config changes live.
    pub fn rearm(&mut self, minutes: u32) {
        *self = Self::new(minutes);
    }

    /// Await the next scheduled tick. If disabled, returns a pending
    /// future that never resolves.
    pub async fn tick(&mut self) {
        match &mut self.interval {
            Some(i) => {
                i.tick().await;
            }
            None => std::future::pending::<()>().await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn disabled_scheduler_never_ticks() {
        let mut s = SyncScheduler::new(0);
        assert!(s.is_disabled());
        let result = tokio::time::timeout(Duration::from_secs(3600), s.tick()).await;
        assert!(result.is_err(), "disabled scheduler must not tick");
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn enabled_scheduler_fires_at_interval() {
        let mut s = SyncScheduler::new(1);
        assert!(!s.is_disabled());
        // First tick: under start_paused, the test runtime auto-advances
        // when no other work is pending.
        let r = tokio::time::timeout(Duration::from_secs(120), s.tick()).await;
        assert!(
            r.is_ok(),
            "1-minute scheduler should tick within 2 minutes of simulated time"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn rearm_updates_minutes() {
        let mut s = SyncScheduler::new(30);
        assert_eq!(s.minutes(), 30);
        s.rearm(60);
        assert_eq!(s.minutes(), 60);
        s.rearm(0);
        assert!(s.is_disabled());
    }
}
