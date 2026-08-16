//! Exponential backoff for blocklist source fetch failures.
//!
//! Implements the backoff sequence defined in PRD section 4.3 (line 360):
//!
//! ```text
//! 1min → 5min → 15min → 1hr → 6hr
//! ```
//!
//! On each consecutive failure the next retry delay advances to the next
//! step in the sequence. Once the final step is reached, all subsequent
//! failures use the maximum delay (6 hours). A successful fetch resets the
//! backoff to the first step.
//!
//! The delays are fixed steps rather than a geometric progression so that
//! the worst-case retry interval is bounded and predictable regardless of
//! how many failures accumulate.

use std::time::Duration;

/// The backoff delay sequence in seconds (1min, 5min, 15min, 1hr, 6hr).
///
/// This matches PRD section 4.3: `1min → 5min → 15min → 1hr → 6hr`.
pub const BACKOFF_SEQUENCE_SECS: [u64; 5] = [60, 300, 900, 3600, 21600];

/// Exponential backoff tracker for a single blocklist source.
///
/// Tracks the current attempt index into [`BACKOFF_SEQUENCE_SECS`] and
/// produces the delay to wait before the next retry. Call [`reset`] on a
/// successful fetch to move back to the first (shortest) step.
///
/// [`reset`]: ExponentialBackoff::reset
#[derive(Debug, Clone)]
pub struct ExponentialBackoff {
    /// Index into `BACKOFF_SEQUENCE_SECS` for the *next* delay to return.
    /// `0` means the first failure has not yet been recorded.
    attempt: u32,
}

impl Default for ExponentialBackoff {
    fn default() -> Self {
        Self::new()
    }
}

impl ExponentialBackoff {
    /// Create a new backoff tracker at the start of the sequence.
    pub fn new() -> Self {
        Self { attempt: 0 }
    }

    /// Record a failure and return the delay to wait before the next retry.
    ///
    /// The first call returns the first step (60s). Each subsequent call
    /// advances one step until the maximum (21600s = 6h) is reached, after
    /// which every call returns the maximum.
    pub fn next_delay(&mut self) -> Duration {
        let idx = (self.attempt as usize).min(BACKOFF_SEQUENCE_SECS.len() - 1);
        let secs = BACKOFF_SEQUENCE_SECS[idx];
        self.attempt = self.attempt.saturating_add(1);
        Duration::from_secs(secs)
    }

    /// Reset the backoff to the start of the sequence.
    ///
    /// Call this after a successful fetch so the next failure starts again
    /// at the shortest delay.
    pub fn reset(&mut self) {
        self.attempt = 0;
    }

    /// Returns the current attempt count (number of consecutive failures
    /// recorded via [`next_delay`]).
    pub fn current_attempt(&self) -> u32 {
        self.attempt
    }

    /// Returns the delay that *would* be returned by the next call to
    /// [`next_delay`], without advancing the attempt counter.
    ///
    /// This is useful for logging/scheduling without consuming a step.
    pub fn peek_next_delay(&self) -> Duration {
        let idx = (self.attempt as usize).min(BACKOFF_SEQUENCE_SECS.len() - 1);
        Duration::from_secs(BACKOFF_SEQUENCE_SECS[idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delay_sequence() {
        let mut bo = ExponentialBackoff::new();
        assert_eq!(bo.next_delay(), Duration::from_secs(60));
        assert_eq!(bo.next_delay(), Duration::from_secs(300));
        assert_eq!(bo.next_delay(), Duration::from_secs(900));
        assert_eq!(bo.next_delay(), Duration::from_secs(3600));
        assert_eq!(bo.next_delay(), Duration::from_secs(21600));
        // All subsequent calls stay at the max.
        assert_eq!(bo.next_delay(), Duration::from_secs(21600));
        assert_eq!(bo.next_delay(), Duration::from_secs(21600));
    }

    #[test]
    fn test_current_attempt() {
        let mut bo = ExponentialBackoff::new();
        assert_eq!(bo.current_attempt(), 0);
        bo.next_delay();
        assert_eq!(bo.current_attempt(), 1);
        bo.next_delay();
        assert_eq!(bo.current_attempt(), 2);
    }

    #[test]
    fn test_reset_on_success() {
        let mut bo = ExponentialBackoff::new();
        bo.next_delay();
        bo.next_delay();
        bo.next_delay();
        assert_eq!(bo.current_attempt(), 3);

        bo.reset();
        assert_eq!(bo.current_attempt(), 0);
        assert_eq!(bo.next_delay(), Duration::from_secs(60));
    }

    #[test]
    fn test_peek_next_delay_does_not_advance() {
        let mut bo = ExponentialBackoff::new();
        assert_eq!(bo.peek_next_delay(), Duration::from_secs(60));
        assert_eq!(bo.current_attempt(), 0);

        bo.next_delay();
        assert_eq!(bo.peek_next_delay(), Duration::from_secs(300));
        assert_eq!(bo.current_attempt(), 1);
    }

    #[test]
    fn test_peek_at_max() {
        let mut bo = ExponentialBackoff::new();
        for _ in 0..10 {
            bo.next_delay();
        }
        assert_eq!(bo.peek_next_delay(), Duration::from_secs(21600));
        assert_eq!(bo.next_delay(), Duration::from_secs(21600));
    }

    #[test]
    fn test_default_is_new() {
        let bo = ExponentialBackoff::default();
        assert_eq!(bo.current_attempt(), 0);
        assert_eq!(bo.peek_next_delay(), Duration::from_secs(60));
    }
}
