//! Reconnect ladder for workspace sync sessions (交互设计 6.3).
//!
//! 2s → 5s → 10s → 30s, then 30s for as long as the server stays away. The
//! supervisor sleeps exactly the value this ladder hands out and publishes the
//! same value as the deadline behind 「离线 — {n} 秒后重试」, so the number the
//! user reads is the number the app is actually waiting on.

use std::time::Duration;

/// The ladder from 6.3, in seconds.
pub const RETRY_LADDER_SECONDS: [u32; 4] = [2, 5, 10, 30];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetryLadder {
    attempt: usize,
}

impl RetryLadder {
    pub const fn new() -> Self {
        Self { attempt: 0 }
    }

    /// Seconds to wait before the `attempt`-th reconnect, capped at the last rung.
    pub fn seconds_for(attempt: usize) -> u32 {
        RETRY_LADDER_SECONDS[attempt.min(RETRY_LADDER_SECONDS.len() - 1)]
    }

    /// Take the next delay and step up the ladder.
    pub fn next_delay(&mut self) -> Duration {
        let seconds = Self::seconds_for(self.attempt);
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs(u64::from(seconds))
    }

    /// Return to the first rung; called once a session has actually connected.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    pub const fn attempt(&self) -> usize {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_matches_the_interaction_spec_and_then_holds_at_thirty() {
        let mut ladder = RetryLadder::new();
        let seconds: Vec<u64> = (0..6).map(|_| ladder.next_delay().as_secs()).collect();
        assert_eq!(seconds, vec![2, 5, 10, 30, 30, 30]);
    }

    #[test]
    fn a_successful_connection_puts_the_next_outage_back_on_the_first_rung() {
        let mut ladder = RetryLadder::new();
        let _ = ladder.next_delay();
        let _ = ladder.next_delay();
        assert_eq!(ladder.attempt(), 2);

        ladder.reset();

        assert_eq!(ladder.next_delay(), Duration::from_secs(2));
    }
}
