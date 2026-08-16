//! Histogram helpers for dnshub latency metrics.
//!
//! These wrap the allocation-free [`metrics`] `histogram!` macro so the DNS
//! handler chain can record latency distributions with a single call. Metric
//! names follow Prometheus conventions: `snake_case` with the `dnshub_` prefix
//! (PRD section 4.6).
//!
//! ## Bucket configuration
//!
//! The Prometheus exporter uses its default buckets
//! (`[0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]` seconds),
//! which cover the DNS latency range (sub-millisecond cache hits through
//! multi-second upstream fallbacks). Callers pass durations in **seconds**
//! (as `f64`), consistent with the `metrics` crate's histogram unit convention.
//!
//! ## Label cardinality
//!
//! Latency histograms use bounded labels only:
//!
//! - `dnshub_upstream_latency_seconds` — labels `tier` (1-5) and `upstream`
//!   (a configured, bounded name such as `unbound`, `odoh`). No raw IPs.
//! - `dnshub_query_duration_seconds` — no labels, so cardinality is 1. This
//!   gives the RED-method "Duration" panel a single overall distribution;
//!   per-dimension breakdowns are available via the upstream histogram.
//!
//! [`metrics`]: https://docs.rs/metrics/latest/metrics/

use metrics::histogram;

/// Record an upstream resolution latency. Observes
/// `dnshub_upstream_latency_seconds` with labels `tier` (the fallback tier
/// index, e.g. `"1"`) and `upstream` (the configured upstream name, e.g.
/// `"unbound"`, `"odoh"`).
///
/// `duration_secs` is the elapsed wall-clock time of the upstream lookup in
/// seconds. Callers should measure from the moment the lookup is dispatched
/// to the upstream until the response (or timeout) is observed.
///
/// Label cardinality is bounded: `tier` is one of 1-5 and `upstream` is a
/// configured name from a small, fixed set (PRD section 4.6).
pub fn record_upstream_latency(tier: &str, upstream: &str, duration_secs: f64) {
    histogram!(
        "dnshub_upstream_latency_seconds",
        "tier" => tier.to_string(),
        "upstream" => upstream.to_string()
    )
    .record(duration_secs);
}

/// Record an end-to-end DNS query duration. Observes
/// `dnshub_query_duration_seconds` (no labels).
///
/// `duration_secs` is the elapsed wall-clock time from query receipt to
/// response send, in seconds. This feeds the RED-method "Duration" panel
/// (PRD section 4.6, Grafana dashboard notes).
pub fn record_query_duration(duration_secs: f64) {
    histogram!("dnshub_query_duration_seconds").record(duration_secs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::test_support::handle;

    #[test]
    fn record_upstream_latency_observes_histogram_with_labels() {
        let _ = handle();
        record_upstream_latency("1", "unbound", 0.012);
        record_upstream_latency("1", "unbound", 0.250);
        record_upstream_latency("2", "odoh", 1.4);

        let out = handle().render();
        assert!(
            out.contains("dnshub_upstream_latency_seconds"),
            "expected upstream latency histogram in render output, got:\n{out}"
        );
        assert!(
            out.contains(r#"tier="1""#),
            "expected tier label `1`, got:\n{out}"
        );
        assert!(
            out.contains(r#"upstream="unbound""#),
            "expected upstream label `unbound`, got:\n{out}"
        );
        assert!(
            out.contains(r#"upstream="odoh""#),
            "expected upstream label `odoh`, got:\n{out}"
        );
        // Histograms emit a `_count` series; verify the count line is present.
        assert!(
            out.contains("dnshub_upstream_latency_seconds_count"),
            "expected histogram count series, got:\n{out}"
        );
    }

    #[test]
    fn record_query_duration_observes_labelless_histogram() {
        let _ = handle();
        record_query_duration(0.005);
        record_query_duration(0.080);

        let out = handle().render();
        assert!(
            out.contains("dnshub_query_duration_seconds"),
            "expected query duration histogram in render output, got:\n{out}"
        );
        assert!(
            out.contains("dnshub_query_duration_seconds_count"),
            "expected histogram count series, got:\n{out}"
        );
    }
}
