//! Reconnect backoff scheduling with saturating exponential delay and injectable jitter.
//!
//! Backoff formula: `min(initial * 2^attempt, maximum)` with ±jitter_percent spread,
//! then clamped to `maximum`. Resets only after a valid Hello and complete SnapshotEnd
//! have been processed.

use std::time::Duration;

/// Default reconnect policy matching the plan: initial 250ms, max 30s, 20% jitter.
const DEFAULT_INITIAL: Duration = Duration::from_millis(250);
const DEFAULT_MAXIMUM: Duration = Duration::from_secs(30);
const DEFAULT_JITTER_PERCENT: u8 = 20;

/// Reconnect policy parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconnectPolicy {
    pub initial: Duration,
    pub maximum: Duration,
    pub jitter_percent: u8,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            initial: DEFAULT_INITIAL,
            maximum: DEFAULT_MAXIMUM,
            jitter_percent: DEFAULT_JITTER_PERCENT,
        }
    }
}

/// A source of jitter for reconnect scheduling. Injectable for deterministic tests.
pub trait JitterSource: Send {
    /// Return a value in `[0, upper]` inclusive.
    fn sample_inclusive(&mut self, upper: u32) -> u32;
}

/// Production jitter source using UUID v4 randomness.
pub struct UuidJitter;

impl JitterSource for UuidJitter {
    fn sample_inclusive(&mut self, upper: u32) -> u32 {
        if upper == 0 {
            return 0;
        }
        // Use UUID v4 bytes as a pseudo-random source.
        let uuid = uuid::Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let raw = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        raw % (upper + 1)
    }
}

/// A reconnect schedule that produces saturating exponential delays with jitter.
pub struct ReconnectSchedule<J: JitterSource> {
    policy: ReconnectPolicy,
    jitter: J,
    attempt: u32,
}

impl<J: JitterSource> ReconnectSchedule<J> {
    pub fn new(policy: ReconnectPolicy, jitter: J) -> Self {
        Self {
            policy,
            jitter,
            attempt: 0,
        }
    }

    /// Compute and return the next reconnect delay, incrementing the attempt counter.
    pub fn next_delay(&mut self) -> Duration {
        // Saturating exponential base.
        let exponent = self.attempt.min(31);
        let base = self
            .policy
            .initial
            .saturating_mul(1_u32 << exponent)
            .min(self.policy.maximum);

        // ±jitter_percent spread around the base.
        let spread = u32::from(self.policy.jitter_percent) * 2;
        let factor_percent =
            100 - u32::from(self.policy.jitter_percent) + self.jitter.sample_inclusive(spread);
        let delay = base.mul_f64(f64::from(factor_percent) / 100.0);

        self.attempt = self.attempt.saturating_add(1);
        delay.min(self.policy.maximum)
    }

    /// Reset the attempt counter to zero. Called only after a valid Hello
    /// plus a fully processed End.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Current attempt count.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FixedJitter {
        values: VecDeque<u32>,
        fallback: u32,
    }

    impl JitterSource for FixedJitter {
        fn sample_inclusive(&mut self, upper: u32) -> u32 {
            let value = self.values.pop_front().unwrap_or(self.fallback);
            assert!(value <= upper);
            value
        }
    }

    #[test]
    fn exponential_jitter_saturates_and_reset_is_explicit() {
        // jitter_percent=20, so spread=40, factor = 80 + sample(40).
        // FixedJitter values: [0, 20, 40] → factors [80, 100, 120]%
        let jitter = FixedJitter {
            values: [0, 20, 40].into(),
            fallback: 40,
        };
        let mut schedule = ReconnectSchedule::new(ReconnectPolicy::default(), jitter);

        // attempt 0: base = 250ms * 1 = 250ms, factor 80% → 200ms
        assert_eq!(schedule.next_delay(), Duration::from_millis(200));
        // attempt 1: base = 250ms * 2 = 500ms, factor 100% → 500ms
        assert_eq!(schedule.next_delay(), Duration::from_millis(500));
        // attempt 2: base = 250ms * 4 = 1000ms, factor 120% → 1200ms
        assert_eq!(schedule.next_delay(), Duration::from_millis(1200));

        // Saturate at 30s.
        for _ in 0..20 {
            let _ = schedule.next_delay();
        }
        assert!(schedule.next_delay() <= Duration::from_secs(30));
        assert!(schedule.attempt() > 20);

        schedule.reset();
        assert_eq!(schedule.attempt(), 0);
    }

    #[test]
    fn zero_jitter_produces_pure_exponential() {
        struct ZeroJitter;
        impl JitterSource for ZeroJitter {
            fn sample_inclusive(&mut self, _upper: u32) -> u32 {
                0
            }
        }
        let policy = ReconnectPolicy {
            initial: Duration::from_millis(100),
            maximum: Duration::from_secs(10),
            jitter_percent: 10,
        };
        let mut schedule = ReconnectSchedule::new(policy, ZeroJitter);
        // factor = 90 + 0 = 90%
        assert_eq!(schedule.next_delay(), Duration::from_millis(90));
        assert_eq!(schedule.next_delay(), Duration::from_millis(180));
    }
}
