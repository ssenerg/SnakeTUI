//! What the server measures about itself.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

const BUCKET_US: u64 = 100;
/// Buckets up to 50 ms; anything later lands in the last one.
const BUCKETS: usize = 500;

/// How late rooms' timers fire, in 100 µs buckets.
pub struct LagHistogram {
    buckets: [AtomicU64; BUCKETS + 1],
}

/// Every room's tick lag, server-wide.
pub static TICK_LAG: LagHistogram = LagHistogram::new();

impl LagHistogram {
    pub const fn new() -> Self {
        Self {
            buckets: [const { AtomicU64::new(0) }; BUCKETS + 1],
        }
    }

    pub fn record(&self, lag: Duration) {
        let i = (lag.as_micros() as u64 / BUCKET_US).min(BUCKETS as u64) as usize;
        self.buckets[i].fetch_add(1, Ordering::Relaxed);
    }

    pub fn count(&self) -> u64 {
        self.buckets.iter().map(|b| b.load(Ordering::Relaxed)).sum()
    }

    /// The lag that a share `q` of samples were at or under, to the bucket
    /// it falls in. `None` before any sample.
    pub fn quantile(&self, q: f64) -> Option<Duration> {
        let total = self.count();
        if total == 0 {
            return None;
        }
        let want = ((total as f64 * q).ceil() as u64).max(1);
        let mut seen = 0;
        for (i, b) in self.buckets.iter().enumerate() {
            seen += b.load(Ordering::Relaxed);
            if seen >= want {
                return Some(Duration::from_micros((i as u64 + 1) * BUCKET_US));
            }
        }
        None
    }

    /// Samples whose bucket ends at or before `limit`.
    pub fn at_or_under(&self, limit: Duration) -> u64 {
        let whole = (limit.as_micros() as u64 / BUCKET_US).min(BUCKETS as u64) as usize;
        self.buckets[..whole]
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .sum()
    }

    /// Every sample added up, each taken as the middle of its bucket.
    pub fn sum(&self) -> Duration {
        let micros: u64 = self
            .buckets
            .iter()
            .enumerate()
            .map(|(i, b)| b.load(Ordering::Relaxed) * (i as u64 * BUCKET_US + BUCKET_US / 2))
            .sum();
        Duration::from_micros(micros)
    }

    pub fn reset(&self) {
        for b in &self.buckets {
            b.store(0, Ordering::Relaxed);
        }
    }
}

impl Default for LagHistogram {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantiles_come_from_the_buckets() {
        let lag = LagHistogram::new();
        assert_eq!(lag.quantile(0.99), None);
        for _ in 0..98 {
            lag.record(Duration::from_micros(50));
        }
        lag.record(Duration::from_millis(3));
        lag.record(Duration::from_secs(2));
        assert_eq!(lag.count(), 100);
        assert_eq!(lag.quantile(0.5), Some(Duration::from_micros(100)));
        assert_eq!(lag.quantile(0.99), Some(Duration::from_micros(3100)));
        assert_eq!(lag.quantile(1.0), Some(Duration::from_micros(50_100)));
        assert_eq!(lag.at_or_under(Duration::from_millis(1)), 98);
        assert_eq!(lag.at_or_under(Duration::from_millis(5)), 99);
        assert_eq!(
            lag.at_or_under(Duration::from_secs(9)),
            99,
            "2 s is past every bucket"
        );
        assert_eq!(lag.sum(), Duration::from_micros(98 * 50 + 3050 + 50_050));
        lag.reset();
        assert_eq!(lag.count(), 0);
    }
}
