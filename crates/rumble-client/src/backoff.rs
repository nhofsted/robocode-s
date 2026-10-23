// SPDX-License-Identifier: EPL-2.0

//! Waiting between retries (§13.3): the server's `Retry-After` plus up to 20% jitter, and
//! exponential backoff for network errors. Randomness is passed in as a number in `[0, 1)`
//! so that tests are deterministic; [`random_unit`] draws one from the OS.

use std::time::Duration;

/// The largest share of a wait that jitter adds.
pub const JITTER: f64 = 0.2;

/// A number in `[0, 1)` from the OS's random source (0.5 if there is none).
pub fn random_unit() -> f64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_err() {
        return 0.5;
    }
    // 53 random bits.
    (u64::from_le_bytes(bytes) >> 11) as f64 / (1u64 << 53) as f64
}

/// `wait` plus `unit` (in `[0, 1]`) times 20% of it.
pub fn jittered(wait: Duration, unit: f64) -> Duration {
    wait.mul_f64(1.0 + JITTER * unit.clamp(0.0, 1.0))
}

/// The wait after a `Retry-After` of the given length.
pub fn retry_after(retry_after: Duration, unit: f64) -> Duration {
    jittered(retry_after, unit)
}

/// Exponential backoff from `min` to `max`, doubling per failure, with jitter. The jittered
/// wait may exceed `max` by up to 20%.
#[derive(Clone, Debug)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    failures: u32,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Backoff {
        Backoff {
            min,
            max,
            failures: 0,
        }
    }

    /// The network-error backoff of the work loop: 5 s to 5 min.
    pub fn network() -> Backoff {
        Backoff::new(Duration::from_secs(5), Duration::from_secs(300))
    }

    /// The backoff for posting results: 2 s to 5 min.
    pub fn results() -> Backoff {
        Backoff::new(Duration::from_secs(2), Duration::from_secs(300))
    }

    /// The wait after one more failure; `unit` is the random number for the jitter.
    pub fn next(&mut self, unit: f64) -> Duration {
        let factor = 1u32.checked_shl(self.failures.min(31)).unwrap_or(u32::MAX);
        self.failures = self.failures.saturating_add(1);
        jittered(self.min.saturating_mul(factor).min(self.max), unit)
    }

    /// After a success.
    pub fn reset(&mut self) {
        self.failures = 0;
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_to_the_cap_without_jitter() {
        let mut b = Backoff::network();
        let waits: Vec<u64> = (0..9).map(|_| b.next(0.0).as_secs()).collect();
        assert_eq!(waits, [5, 10, 20, 40, 80, 160, 300, 300, 300]);
        b.reset();
        assert_eq!(b.next(0.0), Duration::from_secs(5));
    }

    #[test]
    fn results_backoff_starts_at_two_seconds() {
        let mut b = Backoff::results();
        assert_eq!(b.next(0.0), Duration::from_secs(2));
        assert_eq!(b.next(0.0), Duration::from_secs(4));
    }

    #[test]
    fn jitter_is_at_most_twenty_percent() {
        let w = Duration::from_secs(30);
        assert_eq!(retry_after(w, 0.0), w);
        assert_eq!(retry_after(w, 0.5), Duration::from_secs(33));
        assert!(retry_after(w, 0.999_999) < Duration::from_secs(36));
        assert_eq!(retry_after(w, 7.0), Duration::from_secs(36));
    }

    #[test]
    fn many_failures_do_not_overflow() {
        let mut b = Backoff::network();
        for _ in 0..100 {
            assert!(b.next(1.0) <= Duration::from_secs(360));
        }
    }

    #[test]
    fn random_unit_is_in_range() {
        for _ in 0..100 {
            let r = random_unit();
            assert!((0.0..1.0).contains(&r));
        }
    }
}
