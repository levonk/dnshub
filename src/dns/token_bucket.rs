//! Token bucket rate limiter (story 03-004).
//!
//! [`TokenBucket`] implements a classic token bucket: tokens accrue at a
//! configurable `refill_rate` (tokens per second) up to a `capacity` (the
//! burst size). Each call to [`TokenBucket::try_take`] attempts to consume
//! one token; if at least one token is available it is consumed and the call
//! returns `true`, otherwise the call returns `false` and the request is
//! considered rate-limited.
//!
//! The bucket is thread-safe: its mutable state (current token count and the
//! last-refill timestamp) is guarded by a [`parking_lot::Mutex`]. The lock is
//! held only for the duration of the refill + consume calculation, which is a
//! few arithmetic operations, so contention is minimal even under high load.
//!
//! ## Time source
//!
//! Refills are computed from elapsed wall-clock time ([`std::time::Instant`]).
//! The public [`TokenBucket::try_take`] method uses `Instant::now()`, while
//! [`TokenBucket::try_take_at`] accepts an explicit `Instant` so that unit
//! tests can drive the clock deterministically without sleeping.

use parking_lot::Mutex;
use std::time::Instant;

/// A token bucket rate limiter.
///
/// Construct with [`TokenBucket::new`] (full) or
/// [`TokenBucket::with_tokens`] (pre-seeded token count). The bucket starts
/// full (tokens == capacity) which allows an initial burst of `capacity`
/// requests before the refill rate kicks in.
pub struct TokenBucket {
    inner: Mutex<BucketState>,
}

struct BucketState {
    /// Maximum number of tokens the bucket can hold (burst size).
    capacity: f64,
    /// Tokens added per second (requests_per_second).
    refill_rate: f64,
    /// Current token count (fractional tokens are accumulated).
    tokens: f64,
    /// Wall-clock time of the last refill.
    last_refill: Instant,
    /// Wall-clock time of the last successful `try_take`.
    last_seen: Instant,
}

impl TokenBucket {
    /// Create a new token bucket that starts full.
    ///
    /// - `capacity`: the burst size (maximum tokens).
    /// - `refill_rate`: tokens added per second.
    /// - `now`: the instant to seed `last_refill` / `last_seen` with.
    pub fn new(capacity: f64, refill_rate: f64, now: Instant) -> Self {
        Self::with_tokens(capacity, refill_rate, capacity, now)
    }

    /// Create a new token bucket with an explicit initial token count.
    ///
    /// The token count is clamped to `[0, capacity]`.
    pub fn with_tokens(capacity: f64, refill_rate: f64, tokens: f64, now: Instant) -> Self {
        let tokens = tokens.clamp(0.0, capacity);
        Self {
            inner: Mutex::new(BucketState {
                capacity,
                refill_rate,
                tokens,
                last_refill: now,
                last_seen: now,
            }),
        }
    }

    /// The bucket capacity (burst size).
    pub fn capacity(&self) -> f64 {
        self.inner.lock().capacity
    }

    /// The refill rate in tokens per second.
    pub fn refill_rate(&self) -> f64 {
        self.inner.lock().refill_rate
    }

    /// The current number of available tokens (after a refill at `now`).
    ///
    /// This is primarily a testing aid; it performs a refill but does *not*
    /// consume a token.
    pub fn available_tokens_at(&self, now: Instant) -> f64 {
        let mut state = self.inner.lock();
        refill(&mut state, now);
        state.tokens
    }

    /// Attempt to consume one token using the current wall-clock time.
    ///
    /// Returns `true` if a token was consumed (request allowed), `false` if
    /// the bucket is empty (request should be rate-limited).
    pub fn try_take(&self) -> bool {
        self.try_take_at(Instant::now())
    }

    /// Attempt to consume one token at a specific instant.
    ///
    /// Refills the bucket based on the elapsed time since the last refill,
    /// then consumes one token if available. `last_seen` is updated to `now`
    /// regardless of whether a token was consumed, so idle-bucket cleanup
    /// can distinguish active from inactive buckets.
    pub fn try_take_at(&self, now: Instant) -> bool {
        let mut state = self.inner.lock();
        refill(&mut state, now);
        state.last_seen = now;
        if state.tokens >= 1.0 {
            state.tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// The instant of the last successful or attempted `try_take`.
    ///
    /// Used by [`RateLimitHandler`](super::rate_limit::RateLimitHandler) to
    /// evict idle buckets.
    pub fn last_seen(&self) -> Instant {
        self.inner.lock().last_seen
    }
}

/// Refill the bucket based on elapsed time since `state.last_refill`.
///
/// Tokens accrue at `refill_rate` per second and are capped at `capacity`.
/// `last_refill` is advanced to `now`.
fn refill(state: &mut BucketState, now: Instant) {
    if now <= state.last_refill {
        // No time has passed (or clock went backwards); leave tokens as-is.
        return;
    }
    let elapsed = now.duration_since(state.last_refill).as_secs_f64();
    let added = elapsed * state.refill_rate;
    state.tokens = (state.tokens + added).min(state.capacity);
    state.last_refill = now;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn test_burst_capacity_available() {
        let now = t0();
        let bucket = TokenBucket::new(5.0, 1.0, now);
        // A full bucket of capacity 5 should allow 5 consecutive takes.
        for i in 0..5 {
            assert!(
                bucket.try_take_at(now),
                "take {} should succeed within burst capacity",
                i + 1
            );
        }
        // The 6th take (within the same instant) should fail — bucket empty.
        assert!(!bucket.try_take_at(now), "6th take should be refused");
    }

    #[test]
    fn test_exhaustion_then_refusal() {
        let now = t0();
        let bucket = TokenBucket::new(2.0, 1.0, now);
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now), "bucket should be exhausted");
    }

    #[test]
    fn test_refill_over_time() {
        let now = t0();
        // capacity 3, refill 1 token/sec.
        let bucket = TokenBucket::new(3.0, 1.0, now);
        // Exhaust the bucket.
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now));
        // After 2 seconds, 2 tokens should have accrued.
        let later = now + Duration::from_secs(2);
        assert!(bucket.try_take_at(later), "first token after refill");
        assert!(bucket.try_take_at(later), "second token after refill");
        // Only 2 accrued in 2s, so a third take at the same instant fails.
        assert!(
            !bucket.try_take_at(later),
            "no more tokens within the same instant"
        );
    }

    #[test]
    fn test_refill_capped_at_capacity() {
        let now = t0();
        // capacity 3, refill 10 tokens/sec.
        let bucket = TokenBucket::new(3.0, 10.0, now);
        // Drain it.
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now));
        // Wait 10 seconds — refill would add 100 tokens but capacity caps at 3.
        let later = now + Duration::from_secs(10);
        assert!(bucket.available_tokens_at(later) <= 3.0);
        assert!(bucket.try_take_at(later));
        assert!(bucket.try_take_at(later));
        assert!(bucket.try_take_at(later));
        assert!(!bucket.try_take_at(later));
    }

    #[test]
    fn test_recovery_after_idle() {
        let now = t0();
        let bucket = TokenBucket::new(1.0, 1.0, now);
        // Drain the single token.
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now));
        // After 1 second the bucket recovers exactly 1 token.
        let later = now + Duration::from_secs(1);
        assert!(bucket.try_take_at(later), "bucket recovers after idle");
        assert!(!bucket.try_take_at(later));
    }

    #[test]
    fn test_fractional_token_accumulation() {
        let now = t0();
        // capacity 1, refill 2 tokens/sec → 0.2 tokens per 100ms.
        let bucket = TokenBucket::new(1.0, 2.0, now);
        // Drain.
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now));
        // After 100ms only 0.2 tokens have accrued — not enough for a take.
        let ms100 = now + Duration::from_millis(100);
        assert!(
            !bucket.try_take_at(ms100),
            "0.2 tokens should not allow a take"
        );
        // After 600ms total, 1.2 tokens accrued → one take succeeds.
        let ms600 = now + Duration::from_millis(600);
        assert!(bucket.try_take_at(ms600), ">=1 token accrued after 600ms");
    }

    #[test]
    fn test_clock_going_backwards_is_safe() {
        let now = t0();
        let bucket = TokenBucket::new(1.0, 1.0, now);
        // A later instant that is somehow "before" now should not panic or
        // add negative tokens. We simulate by passing the same instant.
        assert!(bucket.try_take_at(now));
        // Same instant again: no refill, bucket empty.
        assert!(!bucket.try_take_at(now));
    }

    #[test]
    fn test_with_tokens_clamps_to_capacity() {
        let now = t0();
        // Request 10 tokens but capacity is 3 → clamped to 3.
        let bucket = TokenBucket::with_tokens(3.0, 1.0, 10.0, now);
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(bucket.try_take_at(now));
        assert!(!bucket.try_take_at(now));
    }

    #[test]
    fn test_last_seen_updated_on_attempt() {
        let now = t0();
        let bucket = TokenBucket::new(1.0, 1.0, now);
        let _ = bucket.try_take_at(now);
        assert_eq!(bucket.last_seen(), now);
        let later = now + Duration::from_secs(5);
        // Even a failed take updates last_seen.
        let _ = bucket.try_take_at(later);
        assert_eq!(bucket.last_seen(), later);
    }
}
