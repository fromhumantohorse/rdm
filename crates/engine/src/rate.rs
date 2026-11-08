//! Global token-bucket speed limiter shared by every connection of every download.

use std::{
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use tokio::sync::Notify;

#[derive(Debug)]
pub struct RateLimit {
    /// Bytes per second; 0 = unlimited (fast path, no lock).
    rate: AtomicU64,
    bucket: Mutex<(Instant, f64)>,
    /// Wakes the connections sleeping off a debt counted at the previous rate.
    changed: Notify,
}

impl Default for RateLimit {
    fn default() -> Self {
        Self { rate: AtomicU64::new(0), bucket: Mutex::new((Instant::now(), 0.0)), changed: Notify::new() }
    }
}

impl RateLimit {
    /// Applied at once: a connection waiting at the old rate (a minute, at a few KiB/s) goes on
    /// under the new one, the debt counted at the old rate forgiven.
    pub fn set(&self, bytes_per_sec: u64) {
        if self.rate.swap(bytes_per_sec, Relaxed) != bytes_per_sec {
            *self.bucket.lock().unwrap_or_else(PoisonError::into_inner) = (Instant::now(), 0.0);
            self.changed.notify_waiters();
        }
    }

    pub fn get(&self) -> u64 {
        self.rate.load(Relaxed)
    }

    /// Accounts for `n` received bytes, sleeping just enough to hold the configured rate.
    pub async fn take(&self, n: usize) {
        // Registered before the debit: a change right after it still ends the wait.
        let changed = self.changed.notified();
        let wait = self.debit(n, Instant::now());
        if !wait.is_zero() {
            tokio::select! {
                () = tokio::time::sleep(wait) => {}
                () = changed => {}
            }
        }
    }

    fn debit(&self, n: usize, now: Instant) -> Duration {
        let rate = self.rate.load(Relaxed);
        if rate == 0 {
            return Duration::ZERO;
        }
        let rate = rate as f64;
        let mut bucket = self.bucket.lock().unwrap_or_else(PoisonError::into_inner);
        let (last, tokens) = *bucket;
        // Burst capped at 1/4 s so the limit is smooth, not bursty.
        let tokens = (tokens + now.saturating_duration_since(last).as_secs_f64() * rate).min(rate / 4.0) - n as f64;
        *bucket = (now, tokens);
        if tokens < 0.0 { Duration::from_secs_f64(-tokens / rate) } else { Duration::ZERO }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_waits() {
        assert_eq!(RateLimit::default().debit(1 << 30, Instant::now()), Duration::ZERO);
    }

    #[test]
    fn debt_turns_into_proportional_wait() {
        let limit = RateLimit::default();
        limit.set(1_000_000);
        let now = Instant::now();
        let wait = limit.debit(2_000_000, now);
        assert!((wait.as_secs_f64() - 2.0).abs() < 0.3, "{wait:?}");
    }

    /// Regression: raising or removing a limit left the connections asleep at the old rate (a
    /// 64 KiB chunk at 1 KiB/s: a minute without a byte).
    #[tokio::test]
    async fn a_new_limit_wakes_the_connections_waiting_at_the_old_one() {
        let limit = std::sync::Arc::new(RateLimit::default());
        limit.set(1024);
        let waiting = tokio::spawn({
            let limit = limit.clone();
            async move { limit.take(64 << 10).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        limit.set(0);

        tokio::time::timeout(Duration::from_secs(5), waiting).await.expect("woken at once").unwrap();
        limit.set(1024);
        assert!(limit.debit(256, Instant::now()) < Duration::from_secs(1), "the old debt (a minute) is forgiven");
    }

    #[test]
    fn the_same_limit_again_keeps_its_debt() {
        let limit = RateLimit::default();
        limit.set(1_000_000);
        limit.debit(2_000_000, Instant::now());

        limit.set(1_000_000); // the settings applied again, unchanged

        assert!(limit.debit(1, Instant::now()) > Duration::from_secs(1), "no burst for an unchanged limit");
    }
}
