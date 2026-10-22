//! In-process sliding-window rate limits and exponential lockout
//! (remote-access.md §10; single instance — ADR-0030).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Sliding-window counter keyed by string.
#[derive(Debug, Default)]
pub struct Windows {
    hits: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Windows {
    /// Records a hit for `key` unless `max` hits already happened within
    /// `window`; `Err(retry_after)` when over the limit.
    ///
    /// # Errors
    /// Over the limit.
    pub fn hit(&self, key: &str, max: usize, window: Duration) -> Result<(), Duration> {
        let now = Instant::now();
        let mut map = self.hits.lock();
        let q = map.entry(key.to_owned()).or_default();
        while q.front().is_some_and(|t| now.duration_since(*t) >= window) {
            q.pop_front();
        }
        if q.len() >= max {
            let oldest = q.front().copied().unwrap_or(now);
            return Err(window.saturating_sub(now.duration_since(oldest)));
        }
        q.push_back(now);
        Ok(())
    }
}

/// Exponential lockout after repeated failures (5 free failures, then
/// 2^(n-5) seconds, capped at 15 minutes).
#[derive(Debug, Default)]
pub struct Lockout {
    state: Mutex<HashMap<String, (u32, Option<Instant>)>>,
}

impl Lockout {
    /// `Err(remaining)` while locked.
    ///
    /// # Errors
    /// Locked.
    pub fn check(&self, key: &str) -> Result<(), Duration> {
        match self.state.lock().get(key) {
            Some((_, Some(until))) if *until > Instant::now() => Err(*until - Instant::now()),
            _ => Ok(()),
        }
    }

    /// Records a failure.
    pub fn fail(&self, key: &str) {
        let mut map = self.state.lock();
        let e = map.entry(key.to_owned()).or_insert((0, None));
        e.0 += 1;
        if e.0 >= 5 {
            let secs = 2u64.saturating_pow(e.0 - 5).min(900);
            e.1 = Some(Instant::now() + Duration::from_secs(secs));
        }
    }

    /// Clears after a success.
    pub fn success(&self, key: &str) {
        self.state.lock().remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_limits() {
        let w = Windows::default();
        for _ in 0..3 {
            w.hit("a", 3, Duration::from_secs(60)).unwrap();
        }
        assert!(w.hit("a", 3, Duration::from_secs(60)).is_err());
        w.hit("b", 3, Duration::from_secs(60)).unwrap();
    }

    #[test]
    fn lockout_after_failures() {
        let l = Lockout::default();
        for _ in 0..4 {
            l.fail("ip");
        }
        assert!(l.check("ip").is_ok());
        l.fail("ip");
        assert!(l.check("ip").is_err());
        l.success("ip");
        assert!(l.check("ip").is_ok());
    }
}
