//! Circuit breaker for blocklist source fetches.
//!
//! Implements the circuit breaker pattern described in PRD section 4.3:
//! after `N` consecutive failures (default 10), the circuit *opens* and
//! fetches are suspended — the daemon serves stale data instead of
//! hammering a failing source. After a configurable cooldown the breaker
//! transitions to *half-open*, allowing a single probe request. If the
//! probe succeeds the breaker closes and normal fetching resumes; if it
//! fails the breaker re-opens for another cooldown period.
//!
//! ## States
//!
//! | State     | Behaviour                                              |
//! |-----------|--------------------------------------------------------|
//! | `Closed`  | Fetches proceed normally. Failures increment a counter.|
//! | `Open`    | Fetches are blocked (`is_open()` returns `true`).      |
//! | `HalfOpen`| A single probe is allowed to test recovery.            |
//!
//! The half-open state is entered lazily: [`is_open`] returns `false` once
//! the cooldown has elapsed, and the caller is expected to attempt one
//! fetch. [`record_success`] / [`record_failure`] then resolve the
//! half-open probe back to `Closed` or `Open` respectively.

use std::time::{Duration, Instant};

/// Circuit breaker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation: fetches proceed, failures are counted.
    Closed,
    /// Tripped: fetches are blocked, stale data is served.
    Open,
    /// Cooldown elapsed: a single probe fetch is allowed.
    HalfOpen,
}

/// Default number of consecutive failures before the breaker opens.
pub const DEFAULT_MAX_FAILURES: u32 = 10;

/// Default cooldown before transitioning from `Open` to `HalfOpen`.
///
/// Set to 5 minutes — long enough to avoid a tight retry loop, short
/// enough to recover promptly once the source is healthy again.
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(300);

/// Circuit breaker for a single blocklist source.
///
/// Tracks consecutive failures and transitions between [`CircuitState`]
/// variants. The breaker is not thread-safe by itself; the daemon wraps it
/// in a per-source mutex (see [`crate::blocklist::health::SourceHealth`]).
#[derive(Debug, Clone)]
pub struct CircuitBreaker {
    state: CircuitState,
    consecutive_failures: u32,
    max_failures: u32,
    cooldown: Duration,
    /// Instant when the `Open` state was entered. Used to compute the
    /// half-open transition. `None` while in `Closed`.
    opened_at: Option<Instant>,
}

impl CircuitBreaker {
    /// Create a new breaker in the `Closed` state with the given threshold
    /// and cooldown.
    pub fn new(max_failures: u32, cooldown: Duration) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            max_failures,
            cooldown,
            opened_at: None,
        }
    }

    /// Create a breaker with the default threshold (10) and cooldown (5min).
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_MAX_FAILURES, DEFAULT_COOLDOWN)
    }

    /// Returns the current state, applying the half-open transition if the
    /// cooldown has elapsed while in the `Open` state.
    pub fn state(&self) -> CircuitState {
        match self.state {
            CircuitState::Open => {
                if let Some(opened) = self.opened_at {
                    if opened.elapsed() >= self.cooldown {
                        return CircuitState::HalfOpen;
                    }
                }
                CircuitState::Open
            }
            other => other,
        }
    }

    /// Returns `true` if the breaker is blocking fetches (i.e. in the `Open`
    /// state and the cooldown has not yet elapsed).
    ///
    /// When this returns `false` the caller may attempt a fetch; if the
    /// effective state is `HalfOpen` that fetch is the recovery probe.
    pub fn is_open(&self) -> bool {
        matches!(self.state(), CircuitState::Open)
    }

    /// Record a successful fetch.
    ///
    /// Resets the consecutive failure counter and closes the circuit
    /// (clearing any half-open probe).
    pub fn record_success(&mut self) {
        let prev = self.state;
        self.consecutive_failures = 0;
        self.opened_at = None;
        self.state = CircuitState::Closed;
        if prev != CircuitState::Closed {
            tracing::info!(
                from = ?prev,
                to = ?CircuitState::Closed,
                "circuit breaker closed after success"
            );
        }
    }

    /// Record a failed fetch.
    ///
    /// In `Closed`: increments the failure counter and opens the circuit
    /// once `max_failures` is reached. In `HalfOpen`: re-opens the circuit
    /// immediately (the probe failed). In `Open`: no-op (already open).
    pub fn record_failure(&mut self) {
        let effective = self.state();
        match effective {
            CircuitState::Closed => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                if self.consecutive_failures >= self.max_failures {
                    self.open();
                }
            }
            CircuitState::HalfOpen => {
                // Probe failed — re-open for another cooldown.
                self.open();
            }
            CircuitState::Open => {
                // Already open; nothing to do.
            }
        }
    }

    /// Force the breaker into the `Open` state and start the cooldown.
    fn open(&mut self) {
        tracing::warn!(
            failures = self.consecutive_failures,
            threshold = self.max_failures,
            "circuit breaker opened"
        );
        self.state = CircuitState::Open;
        self.opened_at = Some(Instant::now());
    }

    /// Returns the number of consecutive failures recorded.
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Returns the configured failure threshold.
    pub fn max_failures(&self) -> u32 {
        self.max_failures
    }

    /// Returns the configured cooldown duration.
    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    /// Returns the instant the breaker opened, if currently in `Open`.
    pub fn opened_at(&self) -> Option<Instant> {
        self.opened_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_starts_closed() {
        let cb = CircuitBreaker::with_defaults();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(!cb.is_open());
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_opens_after_max_failures() {
        let mut cb = CircuitBreaker::new(3, DEFAULT_COOLDOWN);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.is_open());
        assert_eq!(cb.consecutive_failures(), 3);
    }

    #[test]
    fn test_success_closes_from_closed() {
        let mut cb = CircuitBreaker::new(3, DEFAULT_COOLDOWN);
        cb.record_failure();
        cb.record_failure();
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_success_closes_from_open() {
        let mut cb = CircuitBreaker::new(2, DEFAULT_COOLDOWN);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Simulate the half-open transition by recording success directly.
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_half_open_after_cooldown() {
        // Use a very short cooldown so the test doesn't sleep long.
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(10));
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);

        // Wait for cooldown to elapse.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);
        assert!(!cb.is_open());
    }

    #[test]
    fn test_half_open_probe_success_closes() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(10));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures(), 0);
    }

    #[test]
    fn test_half_open_probe_failure_reopens() {
        let mut cb = CircuitBreaker::new(1, Duration::from_millis(10));
        cb.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(cb.state(), CircuitState::HalfOpen);

        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.is_open());
    }

    #[test]
    fn test_failure_in_open_is_noop() {
        let mut cb = CircuitBreaker::new(1, Duration::from_secs(60));
        cb.record_failure();
        assert!(cb.is_open());
        let failures = cb.consecutive_failures();

        // Additional failures while open don't change the counter.
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), failures);
        assert!(cb.is_open());
    }

    #[test]
    fn test_consecutive_failures_reset_on_success() {
        let mut cb = CircuitBreaker::new(5, DEFAULT_COOLDOWN);
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 3);

        cb.record_success();
        assert_eq!(cb.consecutive_failures(), 0);

        // A new failure starts counting from 1 again.
        cb.record_failure();
        assert_eq!(cb.consecutive_failures(), 1);
        assert_eq!(cb.state(), CircuitState::Closed);
    }

    #[test]
    fn test_default_threshold_is_10() {
        let cb = CircuitBreaker::with_defaults();
        assert_eq!(cb.max_failures(), DEFAULT_MAX_FAILURES);
        assert_eq!(cb.max_failures(), 10);
    }
}
