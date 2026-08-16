//! Per-client / global rate limiting middleware (story 03-004).
//!
//! [`RateLimitHandler`] implements the
//! [`DnsMiddleware`](crate::dns::DnsMiddleware) trait and enforces a token
//! bucket rate limit on incoming DNS queries. When `per_client` is enabled
//! each source IP gets its own [`TokenBucket`]; otherwise a single global
//! bucket is shared by all clients. Queries that exceed the configured rate
//! receive a `REFUSED` response ([`MiddlewareAction::Reject(Refused)`]).
//!
//! The handler is constructed from a [`RateLimitConfig`]. When
//! `requests_per_second` is `0` rate limiting is disabled and every request
//! is allowed through (this matches the default config, where the section is
//! effectively a no-op).
//!
//! ## Idle bucket cleanup
//!
//! To prevent unbounded memory growth from churned client IPs, each bucket
//! records the instant it was last touched. [`RateLimitHandler::cleanup`]
//! evicts buckets that have been idle longer than the configured
//! `idle_timeout`. In production this is invoked periodically (e.g. by a
//! background task); the method is exposed publicly so the caller decides the
//! cadence.

use crate::config::RateLimitConfig;
use crate::dns::token_bucket::TokenBucket;
use crate::dns::{DnsMiddleware, MiddlewareAction};
use async_trait::async_trait;
use hickory_proto::op::ResponseCode;
use hickory_server::server::Request;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// Default idle timeout before an unused per-client bucket is evicted.
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// DNS middleware that enforces a token-bucket rate limit.
pub struct RateLimitHandler {
    /// True → one bucket per source IP; false → a single global bucket.
    per_client: bool,
    /// Token bucket capacity (burst size).
    capacity: f64,
    /// Tokens added per second (requests_per_second).
    refill_rate: f64,
    /// Per-client buckets, keyed by source IP. Guarded by a single mutex;
    /// each [`TokenBucket`] has its own internal lock so the map mutex is held
    /// only for the lookup/insert, not for the token consumption.
    buckets: Mutex<HashMap<IpAddr, TokenBucket>>,
    /// The single global bucket used when `per_client` is false.
    global: TokenBucket,
    /// Buckets idle longer than this are evicted by [`Self::cleanup`].
    idle_timeout: Duration,
    /// True when rate limiting is disabled (requests_per_second == 0).
    disabled: bool,
}

impl RateLimitHandler {
    /// Build a rate limit handler from a [`RateLimitConfig`].
    ///
    /// Uses the default idle timeout ([`DEFAULT_IDLE_TIMEOUT`]). When
    /// `requests_per_second` is `0` the handler is disabled and allows all
    /// requests through.
    pub fn from_config(config: &RateLimitConfig) -> Self {
        Self::with_idle_timeout(config, DEFAULT_IDLE_TIMEOUT)
    }

    /// Build a rate limit handler with a custom idle-bucket timeout.
    pub fn with_idle_timeout(config: &RateLimitConfig, idle_timeout: Duration) -> Self {
        let now = Instant::now();
        let rps = config.requests_per_second as f64;
        let burst = config.burst as f64;
        // A burst of 0 with a non-zero rate would make the bucket unable to
        // hold any tokens; clamp to at least 1 so the rate is still enforceable
        // (one request per `1/rps` seconds).
        let capacity = if rps > 0.0 && burst < 1.0 { 1.0 } else { burst };
        let disabled = rps <= 0.0;
        Self {
            per_client: config.per_client,
            capacity,
            refill_rate: rps,
            buckets: Mutex::new(HashMap::new()),
            global: TokenBucket::new(capacity, rps, now),
            idle_timeout,
            disabled,
        }
    }

    /// Build a handler for tests with an explicit `now` to seed bucket clocks
    /// and a custom idle timeout.
    #[cfg(test)]
    fn with_idle_timeout_and_now(
        config: &RateLimitConfig,
        idle_timeout: Duration,
        now: Instant,
    ) -> Self {
        let rps = config.requests_per_second as f64;
        let burst = config.burst as f64;
        let capacity = if rps > 0.0 && burst < 1.0 { 1.0 } else { burst };
        let disabled = rps <= 0.0;
        Self {
            per_client: config.per_client,
            capacity,
            refill_rate: rps,
            buckets: Mutex::new(HashMap::new()),
            global: TokenBucket::new(capacity, rps, now),
            idle_timeout,
            disabled,
        }
    }

    /// Attempt to consume one token for `client_ip`.
    ///
    /// Returns `true` if the request is allowed, `false` if it is
    /// rate-limited. When rate limiting is disabled this always returns
    /// `true`.
    fn allow(&self, client_ip: IpAddr, now: Instant) -> bool {
        if self.disabled {
            return true;
        }
        if self.per_client {
            // Fast path: bucket already exists. We hold the map lock only for
            // the lookup; the token consumption happens under the bucket's
            // own lock.
            let exists = {
                let map = self.buckets.lock();
                map.get(&client_ip).is_some()
            };
            if exists {
                let map = self.buckets.lock();
                let bucket = map.get(&client_ip).expect("bucket present");
                return bucket.try_take_at(now);
            }
            // Slow path: insert a new full bucket for this IP.
            let mut map = self.buckets.lock();
            // Re-check: another thread may have inserted between the fast path
            // and here.
            let bucket = map
                .entry(client_ip)
                .or_insert_with(|| TokenBucket::new(self.capacity, self.refill_rate, now));
            bucket.try_take_at(now)
        } else {
            self.global.try_take_at(now)
        }
    }

    /// Evict per-client buckets that have been idle longer than the idle
    /// timeout.
    ///
    /// Returns the number of buckets removed. This is a no-op when
    /// `per_client` is false (there are no per-client buckets to evict).
    pub fn cleanup(&self, now: Instant) -> usize {
        if !self.per_client {
            return 0;
        }
        let mut map = self.buckets.lock();
        let before = map.len();
        map.retain(|_, bucket| now.duration_since(bucket.last_seen()) < self.idle_timeout);
        let removed = before - map.len();
        if removed > 0 {
            info!(removed, remaining = map.len(), "rate-limit: evicted idle buckets");
        }
        removed
    }

    /// The number of currently-tracked per-client buckets (testing aid).
    pub fn bucket_count(&self) -> usize {
        self.buckets.lock().len()
    }
}

#[async_trait]
impl DnsMiddleware for RateLimitHandler {
    async fn process(&self, request: &Request) -> MiddlewareAction {
        let client_ip = request.src().ip();
        let now = Instant::now();
        if self.allow(client_ip, now) {
            debug!(client_ip = %client_ip, "rate-limit: allow");
            MiddlewareAction::Continue
        } else {
            warn!(client_ip = %client_ip, "rate-limit: refused");
            MiddlewareAction::Reject(ResponseCode::Refused)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Message;
    use hickory_proto::rr::{Name, RecordType};
    use hickory_proto::serialize::binary::BinEncodable;
    use hickory_server::server::Request;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    /// Build a DNS `Request` carrying an A-record query for `domain` from
    /// `src_ip`. Mirrors the helper in `policy/handler.rs`.
    fn make_request(src_ip: IpAddr) -> Request {
        let name = Name::from_utf8("example.com").unwrap();
        let mut msg = Message::new(
            1,
            hickory_proto::op::MessageType::Query,
            hickory_proto::op::OpCode::Query,
        );
        msg.add_query(hickory_proto::op::Query::query(name, RecordType::A));
        let raw = msg.to_bytes().unwrap();
        Request::from_bytes(
            raw,
            SocketAddr::new(src_ip, 12345),
            hickory_server::net::xfer::Protocol::Udp,
        )
        .unwrap()
    }

    fn ip(o1: u8, o2: u8, o3: u8, o4: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(o1, o2, o3, o4))
    }

    // ---- per-client isolation ----

    #[tokio::test]
    async fn test_per_client_allowed_under_limit() {
        let cfg = RateLimitConfig {
            requests_per_second: 1,
            burst: 3,
            per_client: true,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        let client = ip(192, 168, 1, 10);
        // Within burst capacity: all allowed.
        for _ in 0..3 {
            let action = handler.process(&make_request(client)).await;
            assert!(matches!(action, MiddlewareAction::Continue));
        }
    }

    #[tokio::test]
    async fn test_per_client_blocked_over_limit() {
        let cfg = RateLimitConfig {
            requests_per_second: 1,
            burst: 2,
            per_client: true,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        let client = ip(10, 0, 0, 1);
        // Burst of 2 allowed.
        assert!(matches!(
            handler.process(&make_request(client)).await,
            MiddlewareAction::Continue
        ));
        assert!(matches!(
            handler.process(&make_request(client)).await,
            MiddlewareAction::Continue
        ));
        // 3rd request within the same instant is refused.
        assert!(matches!(
            handler.process(&make_request(client)).await,
            MiddlewareAction::Reject(ResponseCode::Refused)
        ));
    }

    #[tokio::test]
    async fn test_per_client_isolation() {
        // Each IP has its own bucket; exhausting one does not affect another.
        let cfg = RateLimitConfig {
            requests_per_second: 1,
            burst: 1,
            per_client: true,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        let a = ip(192, 168, 1, 1);
        let b = ip(192, 168, 1, 2);
        // Drain client A's single token.
        assert!(matches!(
            handler.process(&make_request(a)).await,
            MiddlewareAction::Continue
        ));
        assert!(matches!(
            handler.process(&make_request(a)).await,
            MiddlewareAction::Reject(ResponseCode::Refused)
        ));
        // Client B still has its full bucket.
        assert!(matches!(
            handler.process(&make_request(b)).await,
            MiddlewareAction::Continue
        ));
    }

    // ---- global rate limiting ----

    #[tokio::test]
    async fn test_global_rate_limit() {
        let cfg = RateLimitConfig {
            requests_per_second: 1,
            burst: 2,
            per_client: false,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        // Two requests from different IPs share the global bucket.
        assert!(matches!(
            handler.process(&make_request(ip(10, 0, 0, 1))).await,
            MiddlewareAction::Continue
        ));
        assert!(matches!(
            handler.process(&make_request(ip(10, 0, 0, 2))).await,
            MiddlewareAction::Continue
        ));
        // Third request (any IP) is refused — global bucket exhausted.
        assert!(matches!(
            handler.process(&make_request(ip(10, 0, 0, 3))).await,
            MiddlewareAction::Reject(ResponseCode::Refused)
        ));
    }

    // ---- disabled ----

    #[tokio::test]
    async fn test_disabled_when_rps_zero() {
        let cfg = RateLimitConfig {
            requests_per_second: 0,
            burst: 0,
            per_client: true,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        // Many requests from many IPs all allowed.
        for i in 0..100 {
            let client = ip(10, 0, 0, (i % 254) as u8 + 1);
            assert!(matches!(
                handler.process(&make_request(client)).await,
                MiddlewareAction::Continue
            ));
        }
    }

    // ---- idle bucket cleanup ----

    #[tokio::test]
    async fn test_idle_bucket_cleanup() {
        let cfg = RateLimitConfig {
            requests_per_second: 10,
            burst: 10,
            per_client: true,
        };
        // Use a short idle timeout (500ms) so the test can exercise eviction
        // without sleeping for minutes.
        let now = Instant::now();
        let handler =
            RateLimitHandler::with_idle_timeout_and_now(&cfg, Duration::from_millis(500), now);
        // Touch two clients at `now`.
        handler.allow(ip(192, 168, 1, 1), now);
        handler.allow(ip(192, 168, 1, 2), now);
        assert_eq!(handler.bucket_count(), 2);

        // Touch only client 1 at now + 1s (past the 500ms idle timeout).
        let later = now + Duration::from_secs(1);
        handler.allow(ip(192, 168, 1, 1), later);

        // Client 2 (last seen at `now`, 1s ago) exceeds the 500ms timeout and
        // should be evicted; client 1 (last seen at `later`) is retained.
        let removed = handler.cleanup(later);
        assert_eq!(removed, 1, "idle bucket for client 2 should be evicted");
        assert_eq!(handler.bucket_count(), 1);

        // Client 1 is still present and usable.
        assert!(handler.allow(ip(192, 168, 1, 1), later));
    }

    #[tokio::test]
    async fn test_cleanup_noop_when_not_per_client() {
        let cfg = RateLimitConfig {
            requests_per_second: 10,
            burst: 10,
            per_client: false,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        handler.allow(ip(10, 0, 0, 1), Instant::now());
        assert_eq!(handler.cleanup(Instant::now()), 0);
        assert_eq!(handler.bucket_count(), 0);
    }

    // ---- burst clamp when burst < 1 but rps > 0 ----

    #[tokio::test]
    async fn test_burst_clamped_to_one() {
        let cfg = RateLimitConfig {
            requests_per_second: 1,
            burst: 0,
            per_client: false,
        };
        let handler = RateLimitHandler::from_config(&cfg);
        // capacity clamped to 1 → first request allowed, second refused.
        assert!(matches!(
            handler.process(&make_request(ip(10, 0, 0, 1))).await,
            MiddlewareAction::Continue
        ));
        assert!(matches!(
            handler.process(&make_request(ip(10, 0, 0, 1))).await,
            MiddlewareAction::Reject(ResponseCode::Refused)
        ));
    }
}
