//! Per-provider limit state: circuit breaker, rate-limit / quota windows and
//! latency statistics (spec §5.1). Pure and clock-injected for tests.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Consecutive Server/Network failures within [`WINDOW`] that open the circuit.
pub const THRESHOLD: usize = 5;
/// Failure window.
pub const WINDOW: Duration = Duration::from_secs(60);
/// First open period (doubles up to [`MAX_OPEN`]).
pub const BASE_OPEN: Duration = Duration::from_secs(30);
/// Longest open period.
pub const MAX_OPEN: Duration = Duration::from_secs(300);

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Requests flow.
    Closed,
    /// Requests are rejected until the instant.
    Open {
        /// Re-probe time.
        until: Instant,
    },
    /// One probe request is allowed.
    HalfOpen,
}

/// Circuit breaker (5 failures / 60 s ⇒ open 30 s, doubling up to 5 min;
/// half-open allows one probe).
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    failures: VecDeque<Instant>,
    open_until: Option<Instant>,
    next_open: Duration,
    probing: bool,
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self { failures: VecDeque::new(), open_until: None, next_open: BASE_OPEN, probing: false }
    }
}

impl CircuitBreaker {
    /// Current state.
    #[must_use]
    pub fn state(&self, now: Instant) -> CircuitState {
        match self.open_until {
            Some(until) if now < until => CircuitState::Open { until },
            Some(_) => CircuitState::HalfOpen,
            None => CircuitState::Closed,
        }
    }

    /// Whether a request may be attempted now (half-open admits one probe).
    pub fn allow(&mut self, now: Instant) -> bool {
        match self.state(now) {
            CircuitState::Closed => true,
            CircuitState::Open { .. } => false,
            CircuitState::HalfOpen => {
                if self.probing {
                    false
                } else {
                    self.probing = true;
                    true
                }
            }
        }
    }

    /// Records a circuit-relevant failure. Returns true when the circuit opened now.
    pub fn record_failure(&mut self, now: Instant) -> bool {
        if matches!(self.state(now), CircuitState::HalfOpen) {
            // Failed probe: reopen for longer.
            self.probing = false;
            self.open_until = Some(now + self.next_open);
            self.next_open = (self.next_open * 2).min(MAX_OPEN);
            return true;
        }
        self.failures.push_back(now);
        while self.failures.front().is_some_and(|t| now.duration_since(*t) > WINDOW) {
            self.failures.pop_front();
        }
        if self.failures.len() >= THRESHOLD && self.open_until.is_none() {
            self.failures.clear();
            self.open_until = Some(now + self.next_open);
            self.next_open = (self.next_open * 2).min(MAX_OPEN);
            return true;
        }
        false
    }

    /// Records a success: closes the circuit and resets backoff.
    pub fn record_success(&mut self) {
        *self = Self::default();
    }

    /// Whether the circuit is open right now.
    #[must_use]
    pub fn is_open(&self, now: Instant) -> bool {
        matches!(self.state(now), CircuitState::Open { .. })
    }
}

/// Exponentially weighted latency average.
#[derive(Debug, Clone, Copy, Default)]
pub struct LatencyStats {
    ewma_ms: Option<f64>,
    samples: u64,
}

impl LatencyStats {
    /// Adds a sample.
    pub fn record(&mut self, ms: u64) {
        let ms = ms as f64;
        self.ewma_ms = Some(match self.ewma_ms {
            None => ms,
            Some(prev) => prev * 0.7 + ms * 0.3,
        });
        self.samples += 1;
    }

    /// Average (None before the first sample).
    #[must_use]
    pub fn average_ms(&self) -> Option<f64> {
        self.ewma_ms
    }

    /// Number of samples.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }
}

/// Rate-limit / quota windows of one provider.
#[derive(Debug, Clone, Default)]
pub struct LimitWindows {
    /// Throttled until.
    pub rate_limited_until: Option<Instant>,
    /// Quota exhausted until (unknown resets: 15 min).
    pub quota_until: Option<Instant>,
}

impl LimitWindows {
    /// Default wait when a rate limit carries no retry-after.
    pub const DEFAULT_RATE_WAIT: Duration = Duration::from_secs(20);
    /// Quota back-off when the reset time is unknown.
    pub const DEFAULT_QUOTA_WAIT: Duration = Duration::from_secs(15 * 60);

    /// Seconds until the provider may be used again (None = usable now).
    #[must_use]
    pub fn blocked_for(&self, now: Instant) -> Option<Duration> {
        [self.rate_limited_until, self.quota_until]
            .into_iter()
            .flatten()
            .filter(|t| *t > now)
            .max()
            .map(|t| t.duration_since(now))
    }

    /// Whether the quota window is active.
    #[must_use]
    pub fn quota_exhausted(&self, now: Instant) -> bool {
        self.quota_until.is_some_and(|t| t > now)
    }

    /// Clears both windows (successful call).
    pub fn clear(&mut self) {
        self.rate_limited_until = None;
        self.quota_until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_after_threshold_and_half_opens() {
        let t0 = Instant::now();
        let mut cb = CircuitBreaker::default();
        for i in 0..4 {
            assert!(!cb.record_failure(t0 + Duration::from_secs(i)));
        }
        assert!(cb.allow(t0 + Duration::from_secs(4)));
        assert!(cb.record_failure(t0 + Duration::from_secs(5)), "5th failure opens");
        assert!(cb.is_open(t0 + Duration::from_secs(6)));
        assert!(!cb.allow(t0 + Duration::from_secs(10)));
        // After 30s: half-open, exactly one probe.
        let later = t0 + Duration::from_secs(36);
        assert_eq!(cb.state(later), CircuitState::HalfOpen);
        assert!(cb.allow(later));
        assert!(!cb.allow(later));
        // Probe fails → reopen for 60s (doubled).
        assert!(cb.record_failure(later));
        assert!(cb.is_open(later + Duration::from_secs(59)));
        assert!(!cb.is_open(later + Duration::from_secs(61)));
        cb.record_success();
        assert_eq!(cb.state(later + Duration::from_secs(61)), CircuitState::Closed);
    }

    #[test]
    fn failures_outside_window_do_not_count() {
        let t0 = Instant::now();
        let mut cb = CircuitBreaker::default();
        for i in 0..4 {
            cb.record_failure(t0 + Duration::from_secs(i));
        }
        assert!(!cb.record_failure(t0 + Duration::from_secs(120)));
        assert!(!cb.is_open(t0 + Duration::from_secs(121)));
    }

    #[test]
    fn open_period_caps_at_max() {
        let mut t = Instant::now();
        let mut cb = CircuitBreaker::default();
        for _ in 0..THRESHOLD {
            cb.record_failure(t);
        }
        for _ in 0..10 {
            t += MAX_OPEN + Duration::from_secs(1);
            assert!(cb.allow(t));
            cb.record_failure(t);
        }
        assert!(!cb.is_open(t + MAX_OPEN + Duration::from_millis(1)));
    }

    #[test]
    fn latency_and_windows() {
        let mut l = LatencyStats::default();
        l.record(100);
        l.record(200);
        assert!((l.average_ms().unwrap() - 130.0).abs() < 1e-9);
        let now = Instant::now();
        let mut w = LimitWindows { rate_limited_until: Some(now + Duration::from_secs(5)), quota_until: None };
        assert!(w.blocked_for(now).unwrap() <= Duration::from_secs(5));
        assert!(!w.quota_exhausted(now));
        w.clear();
        assert!(w.blocked_for(now).is_none());
    }
}
