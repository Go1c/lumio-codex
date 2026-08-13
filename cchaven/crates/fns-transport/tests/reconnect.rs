use fns_transport::{JitterSource, ReconnectPolicy, ReconnectSchedule};
use std::collections::VecDeque;
use std::time::Duration;

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
    // jitter_percent=20 → spread=40, factor = 80 + sample(40).
    // FixedJitter: [0, 20, 40] → factors [80%, 100%, 120%]
    let jitter = FixedJitter {
        values: [0, 20, 40].into(),
        fallback: 40,
    };
    let mut schedule = ReconnectSchedule::new(ReconnectPolicy::default(), jitter);

    // attempt 0: 250ms * 1 * 80% = 200ms
    assert_eq!(schedule.next_delay(), Duration::from_millis(200));
    // attempt 1: 250ms * 2 * 100% = 500ms
    assert_eq!(schedule.next_delay(), Duration::from_millis(500));
    // attempt 2: 250ms * 4 * 120% = 1200ms
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
fn delay_never_exceeds_maximum() {
    struct MaxJitter;
    impl JitterSource for MaxJitter {
        fn sample_inclusive(&mut self, upper: u32) -> u32 {
            upper
        }
    }
    let policy = ReconnectPolicy {
        initial: Duration::from_secs(1),
        maximum: Duration::from_millis(500),
        jitter_percent: 50,
    };
    let mut schedule = ReconnectSchedule::new(policy, MaxJitter);
    // Even with max jitter, result should be clamped to maximum.
    for _ in 0..50 {
        assert!(schedule.next_delay() <= Duration::from_millis(500));
    }
}
