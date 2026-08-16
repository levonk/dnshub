//! Per-source health tracking for blocklist fetches.
//!
//! [`SourceHealth`] combines an [`ExponentialBackoff`] and a
//! [`CircuitBreaker`] into a single per-source tracker. It records the
//! timestamp of the last successful fetch, the number of consecutive
//! failures, and the instant after which the next retry is permitted.
//!
//! The daemon consults [`SourceHealth::should_retry`] before each fetch:
//!
//! - If the circuit breaker is **open** and the cooldown has not elapsed,
//!   the fetch is skipped (stale data is served).
//! - Otherwise the fetch proceeds, and [`SourceHealth::record_success`] or
//!   [`SourceHealth::record_failure`] updates the backoff and breaker.
//!
//! Metrics are emitted via the [`metrics`] facade so they appear in the
//! Prometheus scrape output:
//!
//! | Metric                                  | Type    | Labels  |
//! |-----------------------------------------|---------|---------|
//! | `dnshub_blocklist_source_healthy`       | gauge   | `source`|
//! | `dnshub_blocklist_source_failures_total`| counter | `source`|

use std::time::{Duration, Instant};

use crate::blocklist::backoff::ExponentialBackoff;
use crate::blocklist::circuit_breaker::{CircuitBreaker, CircuitState};

/// Per-source health tracker.
///
/// Wraps the backoff and circuit breaker state for a single blocklist
/// source and exposes a simple success/failure API. The daemon holds one
/// of these per configured source.
#[derive(Debug, Clone)]
pub struct SourceHealth {
    /// Source name (for logging and metric labels).
    source_name: String,
    backoff: ExponentialBackoff,
    breaker: CircuitBreaker,
    /// Wall-clock instant of the last successful fetch (`None` until the
    /// first success).
    last_success: Option<Instant>,
    /// Earliest instant at which a retry is permitted. Set on each failure
    /// to `now + next_backoff_delay`. `None` while healthy.
    next_retry_at: Option<Instant>,
}

impl SourceHealth {
    /// Create a new health tracker for the named source.
    pub fn new(source_name: impl Into<String>, max_failures: u32, cooldown: Duration) -> Self {
        let name = source_name.into();
        Self {
            source_name: name,
            backoff: ExponentialBackoff::new(),
            breaker: CircuitBreaker::new(max_failures, cooldown),
            last_success: None,
            next_retry_at: None,
        }
    }

    /// Returns the source name this tracker is bound to.
    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    /// Returns the effective circuit breaker state (applying the half-open
    /// transition if the cooldown has elapsed).
    pub fn state(&self) -> CircuitState {
        self.breaker.state()
    }

    /// Returns `true` if the source is currently considered healthy (the
    /// circuit breaker is `Closed` or `HalfOpen`).
    pub fn is_healthy(&self) -> bool {
        !matches!(self.state(), CircuitState::Open)
    }

    /// Returns `true` if a fetch should be attempted now.
    ///
    /// - **Open**: the cooldown has not elapsed → `false` (serve stale).
    /// - **HalfOpen**: the cooldown elapsed → `true` (allow one probe).
    /// - **Closed**: honour the backoff schedule (`next_retry_at`); if no
    ///   backoff is pending, `true`.
    pub fn should_retry(&self) -> bool {
        match self.breaker.state() {
            CircuitState::Open => false,
            CircuitState::HalfOpen => true,
            CircuitState::Closed => {
                if let Some(next) = self.next_retry_at {
                    Instant::now() >= next
                } else {
                    true
                }
            }
        }
    }

    /// Returns the instant of the last successful fetch, if any.
    pub fn last_success(&self) -> Option<Instant> {
        self.last_success
    }

    /// Returns the number of consecutive failures recorded.
    pub fn consecutive_failures(&self) -> u32 {
        self.breaker.consecutive_failures()
    }

    /// Returns the instant after which the next retry is permitted, if a
    /// backoff is currently scheduled.
    pub fn next_retry_at(&self) -> Option<Instant> {
        self.next_retry_at
    }

    /// Record a successful fetch.
    ///
    /// Resets the backoff, closes the circuit breaker, and records the
    /// healthy gauge. Idempotent with respect to repeated successes.
    pub fn record_success(&mut self) {
        self.backoff.reset();
        self.breaker.record_success();
        self.last_success = Some(Instant::now());
        self.next_retry_at = None;

        tracing::info!(
            source = self.source_name,
            "blocklist source fetch succeeded, health reset"
        );
        emit_health(&self.source_name, true);
    }

    /// Record a failed fetch.
    ///
    /// Advances the backoff, records the failure in the circuit breaker,
    /// schedules the next retry, and increments the failure counter. If the
    /// breaker opens, the source is marked unhealthy.
    pub fn record_failure(&mut self) {
        let delay = self.backoff.next_delay();
        self.breaker.record_failure();
        let now = Instant::now();
        self.next_retry_at = Some(now + delay);

        tracing::warn!(
            source = self.source_name,
            attempt = self.backoff.current_attempt(),
            consecutive_failures = self.breaker.consecutive_failures(),
            next_delay_secs = delay.as_secs(),
            state = ?self.breaker.state(),
            "blocklist source fetch failed, backing off"
        );
        emit_failure(&self.source_name);
        emit_health(&self.source_name, self.is_healthy());
    }

    /// Force a reset to the healthy state (e.g. on a manual reload).
    pub fn reset(&mut self) {
        self.backoff.reset();
        self.breaker.record_success();
        self.next_retry_at = None;
        emit_health(&self.source_name, true);
    }
}

/// Record the per-source healthy gauge (`dnshub_blocklist_source_healthy`).
///
/// `1` = healthy (breaker closed/half-open), `0` = unhealthy (open).
fn emit_health(source: &str, healthy: bool) {
    metrics::gauge!(
        "dnshub_blocklist_source_healthy",
        "source" => source.to_string()
    )
    .set(if healthy { 1.0 } else { 0.0 });
}

/// Increment the per-source failure counter
/// (`dnshub_blocklist_source_failures_total`).
fn emit_failure(source: &str) {
    metrics::counter!(
        "dnshub_blocklist_source_failures_total",
        "source" => source.to_string()
    )
    .increment(1);
}

/// A registry of [`SourceHealth`] trackers, one per configured source.
///
/// The daemon holds one of these and looks up the tracker by source index
/// (matching the order in `BlocklistsConfig::sources`).
#[derive(Debug, Default)]
pub struct SourceHealthRegistry {
    trackers: Vec<SourceHealth>,
}

impl SourceHealthRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a registry with one tracker per source name, using the given
    /// failure threshold and cooldown.
    pub fn for_sources(
        names: impl IntoIterator<Item = String>,
        max_failures: u32,
        cooldown: Duration,
    ) -> Self {
        let trackers = names
            .into_iter()
            .map(|name| SourceHealth::new(name, max_failures, cooldown))
            .collect();
        Self { trackers }
    }

    /// Returns the tracker for the given source index.
    pub fn get(&self, idx: usize) -> Option<&SourceHealth> {
        self.trackers.get(idx)
    }

    /// Returns a mutable reference to the tracker for the given index.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut SourceHealth> {
        self.trackers.get_mut(idx)
    }

    /// Returns the number of trackers in the registry.
    pub fn len(&self) -> usize {
        self.trackers.len()
    }

    /// Returns `true` if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.trackers.is_empty()
    }

    /// Returns an iterator over all trackers.
    pub fn iter(&self) -> impl Iterator<Item = &SourceHealth> {
        self.trackers.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick_health(name: &str) -> SourceHealth {
        SourceHealth::new(name, 3, Duration::from_millis(10))
    }

    #[test]
    fn test_new_source_is_healthy() {
        let h = quick_health("src");
        assert!(h.is_healthy());
        assert!(h.should_retry());
        assert_eq!(h.consecutive_failures(), 0);
        assert!(h.last_success().is_none());
    }

    #[test]
    fn test_failure_schedules_backoff() {
        let mut h = quick_health("src");
        h.record_failure();
        assert!(h.next_retry_at().is_some());
        assert_eq!(h.consecutive_failures(), 1);
        // should_retry is false immediately after a failure (backoff not
        // yet elapsed).
        assert!(!h.should_retry());
    }

    #[test]
    fn test_success_resets_state() {
        let mut h = quick_health("src");
        h.record_failure();
        h.record_failure();
        assert!(!h.is_healthy() || h.consecutive_failures() > 0);

        h.record_success();
        assert!(h.is_healthy());
        assert_eq!(h.consecutive_failures(), 0);
        assert!(h.next_retry_at().is_none());
        assert!(h.last_success().is_some());
        assert!(h.should_retry());
    }

    #[test]
    fn test_circuit_opens_after_threshold() {
        let mut h = quick_health("src"); // threshold 3
        h.record_failure();
        h.record_failure();
        assert!(h.is_healthy(), "still healthy after 2 failures");
        h.record_failure();
        assert!(!h.is_healthy(), "unhealthy after 3 failures");
        assert_eq!(h.state(), CircuitState::Open);
        assert!(!h.should_retry());
    }

    #[test]
    fn test_half_open_allows_retry_after_cooldown() {
        let mut h = quick_health("src"); // cooldown 10ms
        h.record_failure();
        h.record_failure();
        h.record_failure();
        assert!(!h.should_retry());

        // After cooldown, the breaker is half-open and retry is permitted.
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(h.state(), CircuitState::HalfOpen);
        assert!(h.should_retry());
    }

    #[test]
    fn test_half_open_probe_failure_reopens() {
        let mut h = quick_health("src");
        h.record_failure();
        h.record_failure();
        h.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(h.state(), CircuitState::HalfOpen);

        h.record_failure();
        assert_eq!(h.state(), CircuitState::Open);
        assert!(!h.is_healthy());
    }

    #[test]
    fn test_half_open_probe_success_closes() {
        let mut h = quick_health("src");
        h.record_failure();
        h.record_failure();
        h.record_failure();
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(h.state(), CircuitState::HalfOpen);

        h.record_success();
        assert_eq!(h.state(), CircuitState::Closed);
        assert!(h.is_healthy());
        assert_eq!(h.consecutive_failures(), 0);
    }

    #[test]
    fn test_reset_forces_healthy() {
        let mut h = quick_health("src");
        h.record_failure();
        h.record_failure();
        h.record_failure();
        assert!(!h.is_healthy());

        h.reset();
        assert!(h.is_healthy());
        assert!(h.should_retry());
        assert_eq!(h.consecutive_failures(), 0);
    }

    #[test]
    fn test_registry_build_and_lookup() {
        let reg = SourceHealthRegistry::for_sources(
            ["a".to_string(), "b".to_string(), "c".to_string()],
            5,
            Duration::from_secs(60),
        );
        assert_eq!(reg.len(), 3);
        assert!(!reg.is_empty());
        assert_eq!(reg.get(0).unwrap().source_name(), "a");
        assert_eq!(reg.get(2).unwrap().source_name(), "c");
        assert!(reg.get(3).is_none());
    }

    #[test]
    fn test_registry_mut_updates_tracker() {
        let mut reg = SourceHealthRegistry::for_sources(
            ["a".to_string()],
            5,
            Duration::from_secs(60),
        );
        reg.get_mut(0).unwrap().record_failure();
        assert_eq!(reg.get(0).unwrap().consecutive_failures(), 1);
    }

    #[test]
fn test_metrics_recorded_on_failure_and_success() {
    // Use the shared test recorder so the metrics facade is installed.
    crate::metrics::test_support::handle();

    let mut h = SourceHealth::new("metrics-test", 2, Duration::from_secs(60));
    h.record_failure();

    let out = crate::metrics::test_support::handle().render();
    assert!(
        out.contains("dnshub_blocklist_source_failures_total"),
        "expected failures counter in render output:\n{out}"
    );
    assert!(
        out.contains(r#"source="metrics-test""#),
        "expected source label in render output:\n{out}"
    );

    h.record_success();
    let out = crate::metrics::test_support::handle().render();
    assert!(
        out.contains("dnshub_blocklist_source_healthy"),
        "expected healthy gauge in render output:\n{out}"
    );
}
}
