//! Tiered upstream forwarding with per-tier timeout and fallback.
//!
//! [`TieredForwardHandler`] replaces the single-upstream `ForwardingHandler`
//! from story 01-001 with an ordered fallback through upstream tiers
//! (Tier 1: Unbound, Tier 2: dnscrypt ODoH, … Tier 5: Unbound to Root).
//!
//! Each tier wraps a hickory-server [`ForwardZoneHandler`] (the 0.26 name for
//! `ForwardAuthority`) and carries its own per-tier timeout. On a successful
//! lookup the result is returned immediately; on timeout or error the handler
//! logs the failure, records a metric, and falls through to the next tier. If
//! every tier fails, the handler returns `SERVFAIL`.
//!
//! `TieredForwardHandler` implements [`ZoneHandler`] so it can be inserted into
//! a [`Catalog`](hickory_server::zone_handler::Catalog) at the root zone (`.`),
//! exactly where the single-tier forwarder sat. The
//! [`DnshubHandler`](super::DnshubHandler) middleware chain runs *before* the
//! catalog, so tiered forwarding is the terminal step for any query that
//! middleware did not short-circuit.
//!
//! ## Testability
//!
//! The per-tier forward operation is abstracted behind the [`ForwardUpstream`]
//! trait so the tiered fallback logic can be unit-tested with mock upstreams
//! (success, failure, and timeout behaviors) without binding real sockets.

use async_trait::async_trait;
use hickory_proto::op::ResponseCode;
use hickory_proto::rr::{LowerName, Name, RecordType};
use hickory_server::server::{Request, RequestInfo};
use hickory_server::store::forwarder::ForwardZoneHandler;
use hickory_server::zone_handler::{
    AuthLookup, AxfrPolicy, LookupControlFlow, LookupError, LookupOptions, ZoneHandler, ZoneType,
};
use metrics::counter;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// A single forwardable upstream that can be queried for a record.
///
/// Implementations include [`ForwardZoneHandler`] (production) and the mock
/// upstreams used in unit tests. The trait is the seam that makes the tiered
/// fallback logic in [`TieredForwardHandler`] deterministic and testable.
#[async_trait]
pub trait ForwardUpstream: Send + Sync + 'static {
    /// Forward a lookup for `name` / `rtype`.
    ///
    /// Returns `Ok(AuthLookup)` on a successful resolution (the lookup may
    /// still be empty, e.g. NODATA), or `Err(LookupError)` if the upstream
    /// failed or returned an error response code.
    async fn forward_lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
    ) -> Result<AuthLookup, LookupError>;
}

/// Forward a lookup through a hickory-server [`ForwardZoneHandler`].
///
/// This delegates to the `ZoneHandler::lookup` implementation, translating the
/// [`LookupControlFlow`] result into a plain `Result`. `Skip` (the handler did
/// not attempt the query) is treated as a failure so the tier falls through.
#[async_trait]
impl ForwardUpstream for ForwardZoneHandler {
    async fn forward_lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
    ) -> Result<AuthLookup, LookupError> {
        let flow = <Self as ZoneHandler>::lookup(
            self,
            name,
            rtype,
            None,
            LookupOptions::default(),
        )
        .await;
        match flow {
            LookupControlFlow::Continue(Ok(lookup)) | LookupControlFlow::Break(Ok(lookup)) => {
                Ok(lookup)
            }
            LookupControlFlow::Continue(Err(e)) | LookupControlFlow::Break(Err(e)) => Err(e),
            LookupControlFlow::Skip => Err(LookupError::from(ResponseCode::ServFail)),
        }
    }
}

/// One tier in the fallback chain.
///
/// Tiers are queried in ascending `tier` order. `timeout` bounds how long the
/// handler waits for this tier before falling through to the next.
#[derive(Clone)]
pub struct TieredUpstream {
    /// Fallback tier index (1 = primary, 5 = last resort).
    pub tier: u32,
    /// Human-readable upstream name, used in logs and metric labels.
    pub name: String,
    /// Per-tier query timeout.
    pub timeout: Duration,
    /// The forwardable upstream.
    pub upstream: Arc<dyn ForwardUpstream>,
}

impl TieredUpstream {
    /// Build a tier entry.
    pub fn new(tier: u32, name: impl Into<String>, timeout: Duration, upstream: Arc<dyn ForwardUpstream>) -> Self {
        Self {
            tier,
            name: name.into(),
            timeout,
            upstream,
        }
    }
}

/// A [`ZoneHandler`] that forwards queries through upstream tiers in order,
/// with a per-tier timeout and fallback.
///
/// Insert this at the root zone (`.`) in a
/// [`Catalog`](hickory_server::zone_handler::Catalog) to replace the
/// single-tier `ForwardingHandler`. On each query:
///
/// 1. Tiers are tried in ascending `tier` order.
/// 2. Each tier is bounded by its `timeout` via [`tokio::time::timeout`].
/// 3. The first successful lookup is returned immediately.
/// 4. On timeout or error, a failure metric is recorded and the next tier is
///    tried.
/// 5. If all tiers fail, `SERVFAIL` is returned.
pub struct TieredForwardHandler {
    /// Tiers sorted ascending by `tier`.
    tiers: Vec<TieredUpstream>,
    /// Zone origin this handler is registered for (root `.` for a catch-all).
    origin: LowerName,
}

impl TieredForwardHandler {
    /// Build a tiered forward handler from the given (unsorted) tiers.
    ///
    /// The tiers are sorted ascending by `tier` so callers do not need to
    /// order the config themselves. The origin defaults to the root zone.
    pub fn new(mut tiers: Vec<TieredUpstream>) -> Self {
        tiers.sort_by_key(|t| t.tier);
        Self {
            tiers,
            origin: LowerName::from(&Name::root()),
        }
    }

    /// Build a tiered forward handler with an explicit zone origin.
    pub fn with_origin(mut tiers: Vec<TieredUpstream>, origin: LowerName) -> Self {
        tiers.sort_by_key(|t| t.tier);
        Self { tiers, origin }
    }

    /// Returns the configured tiers (sorted).
    pub fn tiers(&self) -> &[TieredUpstream] {
        &self.tiers
    }

    /// Try each tier in order, returning the first successful lookup.
    ///
    /// On timeout or error the failure is logged and recorded, and the next
    /// tier is tried. If every tier fails, `SERVFAIL` is returned.
    async fn forward(
        &self,
        name: &LowerName,
        rtype: RecordType,
    ) -> Result<AuthLookup, LookupError> {
        for tier in &self.tiers {
            counter!(
                "dnshub_tier_queries_total",
                "tier" => tier.tier.to_string(),
                "upstream" => tier.name.clone()
            )
            .increment(1);

            let lookup = tokio::time::timeout(
                tier.timeout,
                tier.upstream.forward_lookup(name, rtype),
            )
            .await;

            match lookup {
                Ok(Ok(result)) => {
                    info!(
                        tier = tier.tier,
                        upstream = %tier.name,
                        name = %name,
                        rtype = %rtype,
                        "tier resolved query"
                    );
                    return Ok(result);
                }
                Ok(Err(e)) => {
                    warn!(
                        tier = tier.tier,
                        upstream = %tier.name,
                        error = %e,
                        "tier failed, falling through to next tier"
                    );
                    record_tier_failure(tier.tier, &tier.name, "error");
                    continue;
                }
                Err(_elapsed) => {
                    warn!(
                        tier = tier.tier,
                        upstream = %tier.name,
                        timeout_ms = tier.timeout.as_millis() as u64,
                        "tier timed out, falling through to next tier"
                    );
                    record_tier_failure(tier.tier, &tier.name, "timeout");
                    continue;
                }
            }
        }

        warn!(name = %name, rtype = %rtype, "all upstream tiers failed");
        Err(LookupError::from(ResponseCode::ServFail))
    }
}

/// Record a per-tier failure with a `reason` label (`"timeout"` or `"error"`).
fn record_tier_failure(tier: u32, upstream: &str, reason: &str) {
    counter!(
        "dnshub_tier_failures_total",
        "tier" => tier.to_string(),
        "upstream" => upstream.to_string(),
        "reason" => reason.to_string()
    )
    .increment(1);
}

#[async_trait]
impl ZoneHandler for TieredForwardHandler {
    fn zone_type(&self) -> ZoneType {
        ZoneType::External
    }

    fn axfr_policy(&self) -> AxfrPolicy {
        AxfrPolicy::Deny
    }

    fn origin(&self) -> &LowerName {
        &self.origin
    }

    async fn lookup(
        &self,
        name: &LowerName,
        rtype: RecordType,
        _request_info: Option<&RequestInfo<'_>>,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        match self.forward(name, rtype).await {
            Ok(lookup) => LookupControlFlow::Continue(Ok(lookup)),
            Err(e) => LookupControlFlow::Continue(Err(e)),
        }
    }

    async fn search(
        &self,
        request: &Request,
        lookup_options: LookupOptions,
    ) -> (LookupControlFlow<AuthLookup>, Option<hickory_proto::rr::TSigResponseContext>) {
        let request_info = match request.request_info() {
            Ok(info) => info,
            Err(e) => return (LookupControlFlow::Break(Err(e)), None),
        };
        let flow = self
            .lookup(
                request_info.query.name(),
                request_info.query.query_type(),
                Some(&request_info),
                lookup_options,
            )
            .await;
        (flow, None)
    }

    async fn nsec_records(
        &self,
        _name: &LowerName,
        _lookup_options: LookupOptions,
    ) -> LookupControlFlow<AuthLookup> {
        LookupControlFlow::Continue(Err(LookupError::from(std::io::Error::other(
            "NSEC records are not available for the tiered forwarder",
        ))))
    }

    fn metrics_label(&self) -> &'static str {
        "tiered_forwarder"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::rr::Name;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A mock upstream with configurable behavior and a call counter.
    struct MockUpstream {
        behavior: MockBehavior,
        calls: Arc<AtomicUsize>,
    }

    enum MockBehavior {
        /// Return an (empty) successful lookup.
        Success,
        /// Return immediately with the given response code.
        Fail(ResponseCode),
        /// Sleep for `Duration` then return success. Used to trigger
        /// `tokio::time::timeout` when the tier timeout is shorter.
        Slow(Duration),
    }

    impl MockUpstream {
        fn new(behavior: MockBehavior) -> (Self, Arc<AtomicUsize>) {
            let calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    behavior,
                    calls: calls.clone(),
                },
                calls,
            )
        }
    }

    #[async_trait]
    impl ForwardUpstream for MockUpstream {
        async fn forward_lookup(
            &self,
            _name: &LowerName,
            _rtype: RecordType,
        ) -> Result<AuthLookup, LookupError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            match &self.behavior {
                MockBehavior::Success => Ok(AuthLookup::default()),
                MockBehavior::Fail(code) => Err(LookupError::from(*code)),
                MockBehavior::Slow(d) => {
                    tokio::time::sleep(*d).await;
                    Ok(AuthLookup::default())
                }
            }
        }
    }

    fn root_name() -> LowerName {
        LowerName::from(&Name::root())
    }

    #[tokio::test]
    async fn tier_1_success_returns_immediately() {
        let (t1, t1_calls) = MockUpstream::new(MockBehavior::Success);
        let (t2, t2_calls) = MockUpstream::new(MockBehavior::Fail(ResponseCode::ServFail));

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(1, "tier1", Duration::from_secs(1), Arc::new(t1)),
            TieredUpstream::new(2, "tier2", Duration::from_secs(1), Arc::new(t2)),
        ]);

        let result = handler.forward(&root_name(), RecordType::A).await;
        assert!(result.is_ok(), "tier 1 success should return Ok");

        assert_eq!(t1_calls.load(Ordering::SeqCst), 1, "tier 1 should be called once");
        assert_eq!(
            t2_calls.load(Ordering::SeqCst),
            0,
            "tier 2 should not be reached when tier 1 succeeds"
        );
    }

    #[tokio::test]
    async fn tier_1_failure_falls_through_to_tier_2() {
        let (t1, t1_calls) = MockUpstream::new(MockBehavior::Fail(ResponseCode::Refused));
        let (t2, t2_calls) = MockUpstream::new(MockBehavior::Success);

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(1, "tier1", Duration::from_secs(1), Arc::new(t1)),
            TieredUpstream::new(2, "tier2", Duration::from_secs(1), Arc::new(t2)),
        ]);

        let result = handler.forward(&root_name(), RecordType::A).await;
        assert!(result.is_ok(), "tier 2 should succeed after tier 1 fails");

        assert_eq!(t1_calls.load(Ordering::SeqCst), 1);
        assert_eq!(t2_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn tier_1_timeout_falls_through_to_tier_2() {
        // Tier 1 sleeps 500ms but its timeout is 50ms, so it times out.
        let (t1, t1_calls) = MockUpstream::new(MockBehavior::Slow(Duration::from_millis(500)));
        let (t2, t2_calls) = MockUpstream::new(MockBehavior::Success);

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(1, "tier1", Duration::from_millis(50), Arc::new(t1)),
            TieredUpstream::new(2, "tier2", Duration::from_secs(1), Arc::new(t2)),
        ]);

        let start = std::time::Instant::now();
        let result = handler.forward(&root_name(), RecordType::A).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok(), "tier 2 should succeed after tier 1 times out");
        assert_eq!(t1_calls.load(Ordering::SeqCst), 1, "tier 1 should be attempted");
        assert_eq!(t2_calls.load(Ordering::SeqCst), 1, "tier 2 should be reached");
        // The fallback should happen around the 50ms tier-1 timeout, well
        // before the 500ms slow sleep would have completed.
        assert!(
            elapsed < Duration::from_millis(400),
            "fallback should not wait for the full slow sleep, took {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn all_tiers_fail_returns_servfail() {
        let (t1, t1_calls) = MockUpstream::new(MockBehavior::Fail(ResponseCode::Refused));
        let (t2, t2_calls) = MockUpstream::new(MockBehavior::Fail(ResponseCode::ServFail));

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(1, "tier1", Duration::from_secs(1), Arc::new(t1)),
            TieredUpstream::new(2, "tier2", Duration::from_secs(1), Arc::new(t2)),
        ]);

        let result = handler.forward(&root_name(), RecordType::A).await;
        assert!(result.is_err(), "all tiers failing should return Err");

        let err = result.unwrap_err();
        assert!(
            matches!(err, LookupError::ResponseCode(ResponseCode::ServFail)),
            "expected SERVFAIL, got {err:?}"
        );

        assert_eq!(t1_calls.load(Ordering::SeqCst), 1);
        assert_eq!(t2_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn all_tiers_timeout_returns_servfail() {
        let (t1, _t1_calls) = MockUpstream::new(MockBehavior::Slow(Duration::from_millis(500)));
        let (t2, _t2_calls) = MockUpstream::new(MockBehavior::Slow(Duration::from_millis(500)));

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(1, "tier1", Duration::from_millis(50), Arc::new(t1)),
            TieredUpstream::new(2, "tier2", Duration::from_millis(50), Arc::new(t2)),
        ]);

        let result = handler.forward(&root_name(), RecordType::A).await;
        assert!(result.is_err());
        assert!(
            matches!(
                result.unwrap_err(),
                LookupError::ResponseCode(ResponseCode::ServFail)
            ),
            "expected SERVFAIL when all tiers time out"
        );
    }

    #[tokio::test]
    async fn tiers_are_sorted_ascending() {
        // Deliberately provided out of order.
        let (t2, t2_calls) = MockUpstream::new(MockBehavior::Success);
        let (t1, t1_calls) = MockUpstream::new(MockBehavior::Success);

        let handler = TieredForwardHandler::new(vec![
            TieredUpstream::new(2, "tier2", Duration::from_secs(1), Arc::new(t2)),
            TieredUpstream::new(1, "tier1", Duration::from_secs(1), Arc::new(t1)),
        ]);

        // Tier 1 should be queried first and short-circuit.
        let _ = handler.forward(&root_name(), RecordType::A).await;

        assert_eq!(handler.tiers()[0].tier, 1);
        assert_eq!(handler.tiers()[1].tier, 2);
        assert_eq!(t1_calls.load(Ordering::SeqCst), 1);
        assert_eq!(t2_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_tier_list_returns_servfail() {
        let handler = TieredForwardHandler::new(vec![]);
        let result = handler.forward(&root_name(), RecordType::A).await;
        assert!(result.is_err());
        assert!(
            matches!(
                result.unwrap_err(),
                LookupError::ResponseCode(ResponseCode::ServFail)
            ),
            "no tiers should yield SERVFAIL"
        );
    }
}
