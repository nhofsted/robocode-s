// SPDX-License-Identifier: EPL-2.0

//! Rate limits in the instance's memory (spec §5.3): token buckets keyed by token hash or
//! viewer IP. Approximate by design: each instance counts on its own.

use std::collections::HashMap;

/// Buckets dropped when the map grows beyond this (full ones first).
const MAX_KEYS: usize = 20_000;

#[derive(Clone, Copy, Debug)]
struct Bucket {
    tokens: f64,
    at_ms: u64,
}

#[derive(Debug, Default)]
pub struct Limiter {
    buckets: HashMap<String, Bucket>,
}

impl Limiter {
    pub fn new() -> Limiter {
        Limiter::default()
    }

    /// Takes one token from `key`'s bucket (`per_second` refill, `burst` capacity). `Err` is
    /// the `Retry-After` in whole seconds (at least 1).
    pub fn check(
        &mut self,
        key: &str,
        per_second: f64,
        burst: f64,
        now_ms: u64,
    ) -> Result<(), u64> {
        if self.buckets.len() >= MAX_KEYS {
            self.buckets.clear();
        }
        let bucket = self.buckets.entry(key.to_string()).or_insert(Bucket {
            tokens: burst,
            at_ms: now_ms,
        });
        let elapsed = now_ms.saturating_sub(bucket.at_ms) as f64 / 1000.0;
        bucket.tokens = (bucket.tokens + elapsed * per_second).min(burst);
        bucket.at_ms = now_ms;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            let wait = (1.0 - bucket.tokens) / per_second;
            Err((wait.ceil() as u64).max(1))
        }
    }

    /// At most one call per `seconds` (a bucket of one).
    pub fn interval(&mut self, key: &str, seconds: u32, now_ms: u64) -> Result<(), u64> {
        if seconds == 0 {
            return Ok(());
        }
        self.check(key, 1.0 / f64::from(seconds), 1.0, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buckets() {
        let mut limiter = Limiter::new();
        for _ in 0..10 {
            assert!(limiter.check("t", 2.0, 10.0, 0).is_ok());
        }
        assert_eq!(limiter.check("t", 2.0, 10.0, 0), Err(1));
        assert!(limiter.check("t", 2.0, 10.0, 500).is_ok());
        assert!(limiter.check("other", 2.0, 10.0, 500).is_ok());
    }

    #[test]
    fn intervals() {
        let mut limiter = Limiter::new();
        assert!(limiter.interval("w", 10, 0).is_ok());
        assert_eq!(limiter.interval("w", 10, 3_000), Err(7));
        assert!(limiter.interval("w", 10, 10_000).is_ok());
        assert!(limiter.interval("w", 0, 10_000).is_ok());
    }
}
