//! Helper functions for recording basic and per-client dnshub metrics.
//!
//! Each helper wraps the allocation-free [`metrics`] facade macros so the DNS
//! handler chain (wired in Phase 02) can record a metric with a single call.
//! Metric names follow Prometheus conventions: `snake_case` with the `dnshub_`
//! prefix (PRD section 4.6).
//!
//! Per-client dimensions use a **client tag** (profile name or hostname, never
//! a raw IP) to keep label cardinality bounded. See [`super::labels`] for the
//! normalization logic.
//!
//! [`metrics`]: https://docs.rs/metrics/latest/metrics/

use metrics::{counter, gauge};

// ---------------------------------------------------------------------------
// Upstream tier usage and latency (PRD section 4.6, lines 1107-1111)
// ---------------------------------------------------------------------------

/// Record a query dispatched to an upstream tier. Increments
/// `dnshub_tier_queries_total` with labels `tier` (e.g. `"1"`..`"5"`) and
/// `upstream` (the configured upstream name, e.g. `"unbound"`, `"odoh"`).
///
/// The PRD minimum label set is `tier`; the `upstream` label is a bounded
/// superset (a small, configured set of names) that preserves the existing
/// per-upstream breakdown without unbounded cardinality.
pub fn record_tier_query(tier: u32, upstream: &str) {
    counter!(
        "dnshub_tier_queries_total",
        "tier" => tier.to_string(),
        "upstream" => upstream.to_string()
    )
    .increment(1);
}

/// Record a per-tier upstream failure. Increments `dnshub_tier_failures_total`
/// with labels `tier`, `upstream`, and `reason` (`"timeout"` or `"error"`).
///
/// The PRD minimum label set is `tier`; the `upstream` and `reason` labels are
/// bounded supersets that keep the existing failure breakdown.
pub fn record_tier_failure(tier: u32, upstream: &str, reason: &str) {
    counter!(
        "dnshub_tier_failures_total",
        "tier" => tier.to_string(),
        "upstream" => upstream.to_string(),
        "reason" => reason.to_string()
    )
    .increment(1);
}

// ---------------------------------------------------------------------------
// DNSSEC validation (PRD section 4.6, line 1114)
// ---------------------------------------------------------------------------

/// Record a DNSSEC validation result. Increments
/// `dnshub_dnssec_validation_total` with label `result` (one of `"valid"`,
/// `"bogus"`, `"indeterminate"`).
///
/// These results are derived from Unbound responses (the validating upstream).
/// The label set is fixed at three values, so cardinality is bounded.
pub fn record_dnssec_validation(result: &str) {
    counter!(
        "dnshub_dnssec_validation_total",
        "result" => result.to_string()
    )
    .increment(1);
}

// ---------------------------------------------------------------------------
// Blocklist daemon metrics (PRD section 4.6, lines 1104, 1123-1127)
// ---------------------------------------------------------------------------

/// Record a blocklist source refresh outcome. Increments
/// `dnshub_blocklist_refresh_total` with labels `source` (the source name,
/// e.g. `"easylist"`, `"hagezi"`) and `status` (one of `"success"`,
/// `"failure"`, `"stale"`).
///
/// `"stale"` is recorded when a refresh fails and the daemon continues serving
/// the previously compiled database (PRD section 4.3, serve-stale on fetch
/// failure).
pub fn record_blocklist_refresh(source: &str, status: &str) {
    counter!(
        "dnshub_blocklist_refresh_total",
        "source" => source.to_string(),
        "status" => status.to_string()
    )
    .increment(1);
}

/// Set the last successful refresh timestamp for a blocklist source. Sets the
/// `dnshub_blocklist_last_refresh_timestamp` gauge with label `source`.
///
/// `timestamp` is a Unix timestamp (seconds since epoch). Use
/// `SystemTime::now().duration_since(UNIX_EPOCH).as_secs() as f64` at the
/// call site.
pub fn set_blocklist_last_refresh(source: &str, timestamp: f64) {
    gauge!(
        "dnshub_blocklist_last_refresh_timestamp",
        "source" => source.to_string()
    )
    .set(timestamp);
}

/// Record a blocklist hot-swap outcome. Increments
/// `dnshub_blocklist_hot_swap_total` with label `status` (one of `"success"`,
/// `"failure"`).
///
/// A hot-swap is the atomic replacement of the live LMDB database handle
/// (PRD section 4.2).
pub fn record_blocklist_hot_swap(status: &str) {
    counter!(
        "dnshub_blocklist_hot_swap_total",
        "status" => status.to_string()
    )
    .increment(1);
}

/// Set the number of entries contributed by a blocklist source. Sets the
/// `dnshub_blocklist_entries_total` gauge with label `source`.
///
/// This is the per-source entry count after parsing and deduplication, used
/// by the blocklist analytics dashboard (PRD section 4.6, line 1104).
pub fn set_blocklist_entries(source: &str, count: usize) {
    gauge!(
        "dnshub_blocklist_entries_total",
        "source" => source.to_string()
    )
    .set(count as f64);
}

// ---------------------------------------------------------------------------
// DHCP lease gauge (PRD section 4.4 — DHCP server observability)
// ---------------------------------------------------------------------------

/// Set the current number of active DHCP leases. Sets the
/// `dnshub_dhcp_leases_active` gauge.
///
/// "Active" leases are those that have not expired or been released. The value
/// is bounded by the configured lease pool size, so cardinality is not a
/// concern.
pub fn set_dhcp_leases_active(count: usize) {
    gauge!("dnshub_dhcp_leases_active").set(count as f64);
}

// ---------------------------------------------------------------------------
// DoT / DoH connection counters (PRD section 4.7 — encrypted transport)
// ---------------------------------------------------------------------------

/// Record an accepted DoT (DNS-over-TLS, port 853) connection. Increments
/// `dnshub_dot_connections_total`.
pub fn record_dot_connection() {
    counter!("dnshub_dot_connections_total").increment(1);
}

/// Record an accepted DoH (DNS-over-HTTPS, port 443) connection. Increments
/// `dnshub_doh_connections_total`.
pub fn record_doh_connection() {
    counter!("dnshub_doh_connections_total").increment(1);
}

// ---------------------------------------------------------------------------
// Query rate
// ---------------------------------------------------------------------------

/// Record a DNS query. Increments `dnshub_queries_total` with labels
/// `client` (the client tag / profile name, never a raw IP) and `qtype`
/// (e.g. `"A"`, `"AAAA"`, `"MX"`).
///
/// Label cardinality is bounded by using `client_tag` rather than raw client
/// IPs (PRD section 4.6). Callers should obtain `client_tag` from
/// [`super::labels::client_to_tag`].
pub fn record_query(client_tag: &str, qtype: &str) {
    counter!("dnshub_queries_total", "client" => client_tag.to_string(), "qtype" => qtype.to_string()).increment(1);
}

/// Record a DNS query attributed to a specific client. Increments
/// `dnshub_queries_total` with labels `client` (the normalized client tag)
/// and `qtype`.
///
/// This is a convenience wrapper that normalizes the client identity via
/// [`super::labels::client_to_tag`] before recording, so callers can pass raw
/// IP/hostname/profile tuples without worrying about label hygiene.
pub fn record_query_per_client(
    ip: &str,
    hostname: Option<&str>,
    profile: Option<&str>,
    qtype: &str,
) {
    let client_tag = super::labels::client_to_tag(ip, hostname, profile);
    record_query(&client_tag, qtype);
}

// ---------------------------------------------------------------------------
// Cache performance
// ---------------------------------------------------------------------------

/// Record a cache hit. Increments `dnshub_cache_hits_total`.
pub fn record_cache_hit() {
    counter!("dnshub_cache_hits_total").increment(1);
}

/// Record a cache miss. Increments `dnshub_cache_misses_total`.
pub fn record_cache_miss() {
    counter!("dnshub_cache_misses_total").increment(1);
}

/// Set the current number of entries in the response cache. Sets the
/// `dnshub_cache_size_entries` gauge.
pub fn set_cache_size(entries: usize) {
    gauge!("dnshub_cache_size_entries").set(entries as f64);
}

/// Set the current cache hit ratio (0.0..=1.0). Sets the
/// `dnshub_cache_hit_ratio` gauge.
pub fn set_cache_hit_ratio(ratio: f64) {
    gauge!("dnshub_cache_hit_ratio").set(ratio);
}

// ---------------------------------------------------------------------------
// Blocklist analytics
// ---------------------------------------------------------------------------

/// Record a blocklist hit. Increments `dnshub_blocklist_hits_total` with
/// labels `category` (e.g. `"ads"`, `"tracker"`, `"malware"`) and `source`
/// (e.g. `"easylist"`, `"hagezi"`, `"stevenblack"`).
pub fn record_blocklist_hit(category: &str, source: &str) {
    counter!("dnshub_blocklist_hits_total", "category" => category.to_string(), "source" => source.to_string())
        .increment(1);
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Record an error. Increments `dnshub_errors_total` with labels `error_type`
/// (e.g. `"SERVFAIL"`, `"NXDOMAIN"`, `"timeout"`, `"refused"`) and `tier`
/// (e.g. `"cache"`, `"upstream"`, `"blocklist"`).
pub fn record_error(error_type: &str, tier: &str) {
    counter!("dnshub_errors_total", "error_type" => error_type.to_string(), "tier" => tier.to_string()).increment(1);
}

// ---------------------------------------------------------------------------
// Per-client policy decisions (PRD section 4.6)
// ---------------------------------------------------------------------------

/// Record a per-client policy decision. Increments
/// `dnshub_policy_decisions_total` with labels `client` (the client tag),
/// `profile` (the profile name), and `decision` (one of `"allowed"`,
/// `"blocked"`, `"redirected"`).
///
/// The `client` label is normalized via [`super::labels::client_to_tag`] so
/// raw IPs never appear in the scrape output. The `profile` label is
/// normalized via [`super::labels::normalize_label`].
///
/// # Arguments
///
/// * `ip` — Raw client IP (never emitted).
/// * `hostname` — Client hostname, if known.
/// * `profile` — Assigned profile name, if any.
/// * `decision` — The policy decision: `"allowed"`, `"blocked"`, or
///   `"redirected"`.
pub fn record_policy_decision(
    ip: &str,
    hostname: Option<&str>,
    profile: Option<&str>,
    decision: &str,
) {
    let client_tag = super::labels::client_to_tag(ip, hostname, profile);
    let profile_label = super::labels::normalize_label(profile);
    counter!(
        "dnshub_policy_decisions_total",
        "client" => client_tag,
        "profile" => profile_label,
        "decision" => decision.to_string()
    )
    .increment(1);
}

/// Record a per-client block decision. Increments
/// `dnshub_policy_decisions_total` with `decision = "blocked"` and the
/// normalized client/profile labels.
///
/// Convenience wrapper around [`record_policy_decision`].
pub fn record_block_per_client(ip: &str, hostname: Option<&str>, profile: Option<&str>) {
    record_policy_decision(ip, hostname, profile, "blocked");
}

/// Record a per-client upstream error. Increments `dnshub_errors_total` with
/// labels `error_type` and `tier = "upstream"`, plus a `client` label
/// normalized via [`super::labels::client_to_tag`].
///
/// This extends the basic [`record_error`] with a per-client dimension while
/// keeping the existing `error_type`/`tier` labels intact.
pub fn record_upstream_error_per_client(
    ip: &str,
    hostname: Option<&str>,
    profile: Option<&str>,
    error_type: &str,
) {
    let client_tag = super::labels::client_to_tag(ip, hostname, profile);
    counter!(
        "dnshub_errors_total",
        "client" => client_tag,
        "error_type" => error_type.to_string(),
        "tier" => "upstream".to_string()
    )
    .increment(1);
}

// ---------------------------------------------------------------------------
// Active client gauge
// ---------------------------------------------------------------------------

/// Set the current number of active clients. Sets the
/// `dnshub_clients_active` gauge.
///
/// "Active" is defined as clients that have issued at least one query within
/// the current reporting window. The value is bounded by the DHCP lease
/// table, so cardinality is not a concern.
pub fn set_client_active(count: usize) {
    gauge!("dnshub_clients_active").set(count as f64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::test_support::handle;

    /// Assert that `render` output contains a line matching the given metric
    /// name and label fragment, after touching the metric.
    fn assert_metric_visible(name: &str, label_fragment: &str) {
        let out = handle().render();
        assert!(
            out.contains(name),
            "expected render output to contain `{name}`, got:\n{out}"
        );
        if !label_fragment.is_empty() {
            assert!(
                out.contains(label_fragment),
                "expected render output to contain `{label_fragment}`, got:\n{out}"
            );
        }
    }

    #[test]
    fn record_query_registers_counter_with_labels() {
        let _ = handle(); // ensure global recorder is installed before recording
        record_query("laptop", "A");
        record_query("laptop", "A");
        record_query("phone", "AAAA");
        assert_metric_visible("dnshub_queries_total", r#"client="laptop""#);
        assert_metric_visible("dnshub_queries_total", r#"qtype="AAAA""#);
    }

    #[test]
    fn record_cache_hit_and_miss_register_counters() {
        let _ = handle();
        record_cache_hit();
        record_cache_miss();
        record_cache_miss();
        assert_metric_visible("dnshub_cache_hits_total", "");
        assert_metric_visible("dnshub_cache_misses_total", "");
    }

    #[test]
    fn set_cache_size_sets_gauge() {
        let _ = handle();
        set_cache_size(42_000);
        let out = handle().render();
        assert!(
            out.contains("dnshub_cache_size_entries 42000"),
            "expected gauge value 42000, got:\n{out}"
        );
    }

    #[test]
    fn set_cache_hit_ratio_sets_gauge() {
        let _ = handle();
        set_cache_hit_ratio(0.75);
        let out = handle().render();
        assert!(
            out.contains("dnshub_cache_hit_ratio"),
            "expected render output to contain the ratio gauge, got:\n{out}"
        );
    }

    #[test]
    fn record_blocklist_hit_registers_counter_with_labels() {
        let _ = handle();
        record_blocklist_hit("ads", "easylist");
        record_blocklist_hit("malware", "urlhaus");
        assert_metric_visible("dnshub_blocklist_hits_total", r#"category="ads""#);
        assert_metric_visible("dnshub_blocklist_hits_total", r#"source="urlhaus""#);
    }

    #[test]
    fn record_error_registers_counter_with_labels() {
        let _ = handle();
        record_error("SERVFAIL", "upstream");
        record_error("timeout", "upstream");
        assert_metric_visible("dnshub_errors_total", r#"error_type="SERVFAIL""#);
        assert_metric_visible("dnshub_errors_total", r#"tier="upstream""#);
    }

    #[test]
    fn record_query_per_client_normalizes_tag() {
        let _ = handle();
        // Profile takes priority over hostname.
        record_query_per_client("192.168.1.50", Some("laptop"), Some("kids"), "A");
        // Hostname used when no profile.
        record_query_per_client("192.168.1.51", Some("phone"), None, "AAAA");
        // Unknown when neither is available.
        record_query_per_client("192.168.1.52", None, None, "MX");

        let out = handle().render();
        assert!(
            out.contains(r#"dnshub_queries_total{client="kids",qtype="A"}"#),
            "expected normalized client tag `kids`, got:\n{out}"
        );
        assert!(
            out.contains(r#"dnshub_queries_total{client="phone",qtype="AAAA"}"#),
            "expected normalized client tag `phone`, got:\n{out}"
        );
        assert!(
            out.contains(r#"dnshub_queries_total{client="unknown",qtype="MX"}"#),
            "expected normalized client tag `unknown`, got:\n{out}"
        );
        // Raw IP must never appear as a label value.
        assert!(
            !out.contains("192.168.1.50"),
            "raw IP must not appear in metric labels, got:\n{out}"
        );
    }

    #[test]
    fn record_policy_decision_registers_counter_with_labels() {
        let _ = handle();
        record_policy_decision("10.0.0.1", Some("laptop"), Some("kids"), "allowed");
        record_policy_decision("10.0.0.2", Some("phone"), Some("kids"), "blocked");
        record_policy_decision("10.0.0.3", Some("tablet"), Some("guest"), "redirected");

        let out = handle().render();
        assert!(
            out.contains("dnshub_policy_decisions_total"),
            "expected policy decisions counter, got:\n{out}"
        );
        assert!(
            out.contains(r#"client="kids""#),
            "expected client label `kids`, got:\n{out}"
        );
        assert!(
            out.contains(r#"profile="kids""#),
            "expected profile label `kids`, got:\n{out}"
        );
        assert!(
            out.contains(r#"decision="allowed""#),
            "expected decision label `allowed`, got:\n{out}"
        );
        assert!(
            out.contains(r#"decision="blocked""#),
            "expected decision label `blocked`, got:\n{out}"
        );
        assert!(
            out.contains(r#"decision="redirected""#),
            "expected decision label `redirected`, got:\n{out}"
        );
        // Raw IPs must not leak.
        assert!(
            !out.contains("10.0.0.1"),
            "raw IP must not appear in policy decision labels, got:\n{out}"
        );
    }

    #[test]
    fn record_policy_decision_unknown_client_uses_sentinel() {
        let _ = handle();
        record_policy_decision("10.0.0.99", None, None, "allowed");
        let out = handle().render();
        assert!(
            out.contains(r#"client="unknown""#),
            "expected `unknown` client tag, got:\n{out}"
        );
        assert!(
            out.contains(r#"profile="unknown""#),
            "expected `unknown` profile label, got:\n{out}"
        );
    }

    #[test]
    fn record_block_per_client_records_blocked_decision() {
        let _ = handle();
        record_block_per_client("10.0.0.5", Some("tv"), Some("iot"));
        let out = handle().render();
        assert!(
            out.contains(r#"dnshub_policy_decisions_total{client="iot",profile="iot",decision="blocked"}"#),
            "expected blocked decision for iot client, got:\n{out}"
        );
    }

    #[test]
    fn record_upstream_error_per_client_registers_with_client_label() {
        let _ = handle();
        record_upstream_error_per_client("10.0.0.10", Some("desktop"), Some("parents"), "timeout");
        let out = handle().render();
        assert!(
            out.contains("dnshub_errors_total"),
            "expected errors counter, got:\n{out}"
        );
        assert!(
            out.contains(r#"client="parents""#),
            "expected client label `parents`, got:\n{out}"
        );
        assert!(
            out.contains(r#"error_type="timeout""#),
            "expected error_type label `timeout`, got:\n{out}"
        );
        assert!(
            out.contains(r#"tier="upstream""#),
            "expected tier label `upstream`, got:\n{out}"
        );
        assert!(
            !out.contains("10.0.0.10"),
            "raw IP must not appear in upstream error labels, got:\n{out}"
        );
    }

    #[test]
    fn set_client_active_sets_gauge() {
        let _ = handle();
        set_client_active(7);
        let out = handle().render();
        assert!(
            out.contains("dnshub_clients_active 7"),
            "expected gauge value 7, got:\n{out}"
        );
        // Verify zero also works (last write wins for gauges).
        set_client_active(0);
        let out = handle().render();
        assert!(
            out.contains("dnshub_clients_active 0"),
            "expected gauge value 0, got:\n{out}"
        );
    }

    // -----------------------------------------------------------------------
    // New metrics from story 05-001
    // -----------------------------------------------------------------------

    #[test]
    fn record_tier_query_registers_counter_with_labels() {
        let _ = handle();
        record_tier_query(1, "unbound");
        record_tier_query(2, "odoh");
        record_tier_query(1, "unbound");
        assert_metric_visible("dnshub_tier_queries_total", r#"tier="1""#);
        assert_metric_visible("dnshub_tier_queries_total", r#"upstream="odoh""#);
    }

    #[test]
    fn record_tier_failure_registers_counter_with_labels() {
        let _ = handle();
        record_tier_failure(1, "unbound", "timeout");
        record_tier_failure(2, "odoh", "error");
        assert_metric_visible("dnshub_tier_failures_total", r#"tier="2""#);
        assert_metric_visible("dnshub_tier_failures_total", r#"reason="timeout""#);
    }

    #[test]
    fn record_dnssec_validation_registers_counter_with_result_label() {
        let _ = handle();
        record_dnssec_validation("valid");
        record_dnssec_validation("valid");
        record_dnssec_validation("bogus");
        record_dnssec_validation("indeterminate");
        assert_metric_visible("dnshub_dnssec_validation_total", r#"result="valid""#);
        assert_metric_visible("dnshub_dnssec_validation_total", r#"result="bogus""#);
        assert_metric_visible(
            "dnshub_dnssec_validation_total",
            r#"result="indeterminate""#,
        );
    }

    #[test]
    fn record_blocklist_refresh_registers_counter_with_labels() {
        let _ = handle();
        record_blocklist_refresh("easylist", "success");
        record_blocklist_refresh("hagezi", "failure");
        record_blocklist_refresh("urlhaus", "stale");
        assert_metric_visible(
            "dnshub_blocklist_refresh_total",
            r#"source="easylist",status="success""#,
        );
        assert_metric_visible(
            "dnshub_blocklist_refresh_total",
            r#"source="hagezi",status="failure""#,
        );
        assert_metric_visible(
            "dnshub_blocklist_refresh_total",
            r#"status="stale""#,
        );
    }

    #[test]
    fn set_blocklist_last_refresh_sets_gauge_with_source_label() {
        let _ = handle();
        set_blocklist_last_refresh("easylist", 1_700_000_000.0);
        let out = handle().render();
        assert!(
            out.contains("dnshub_blocklist_last_refresh_timestamp"),
            "expected last refresh timestamp gauge, got:\n{out}"
        );
        assert!(
            out.contains(r#"source="easylist""#),
            "expected source label `easylist`, got:\n{out}"
        );
        assert!(
            out.contains("1700000000"),
            "expected timestamp value 1700000000, got:\n{out}"
        );
    }

    #[test]
    fn record_blocklist_hot_swap_registers_counter_with_status_label() {
        let _ = handle();
        record_blocklist_hot_swap("success");
        record_blocklist_hot_swap("failure");
        assert_metric_visible(
            "dnshub_blocklist_hot_swap_total",
            r#"status="success""#,
        );
        assert_metric_visible(
            "dnshub_blocklist_hot_swap_total",
            r#"status="failure""#,
        );
    }

    #[test]
    fn set_blocklist_entries_sets_gauge_with_source_label() {
        let _ = handle();
        set_blocklist_entries("easylist", 125_000);
        set_blocklist_entries("hagezi", 80_000);
        let out = handle().render();
        assert!(
            out.contains("dnshub_blocklist_entries_total"),
            "expected blocklist entries gauge, got:\n{out}"
        );
        assert!(
            out.contains(r#"source="easylist""#),
            "expected source label `easylist`, got:\n{out}"
        );
        assert!(
            out.contains("125000"),
            "expected entry count 125000, got:\n{out}"
        );
    }

    #[test]
    fn set_dhcp_leases_active_sets_gauge() {
        let _ = handle();
        set_dhcp_leases_active(42);
        let out = handle().render();
        assert!(
            out.contains("dnshub_dhcp_leases_active 42"),
            "expected gauge value 42, got:\n{out}"
        );
        set_dhcp_leases_active(0);
        let out = handle().render();
        assert!(
            out.contains("dnshub_dhcp_leases_active 0"),
            "expected gauge value 0, got:\n{out}"
        );
    }

    #[test]
    fn record_dot_and_doh_connection_counters() {
        let _ = handle();
        record_dot_connection();
        record_dot_connection();
        record_doh_connection();
        assert_metric_visible("dnshub_dot_connections_total", "");
        assert_metric_visible("dnshub_doh_connections_total", "");
    }
}
