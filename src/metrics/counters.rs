//! Helper functions for recording basic dnshub metrics.
//!
//! Each helper wraps the allocation-free [`metrics`] facade macros so the DNS
//! handler chain (wired in Phase 02) can record a metric with a single call.
//! Metric names follow Prometheus conventions: `snake_case` with the `dnshub_`
//! prefix (PRD section 4.6).
//!
//! These helpers are designed for extensibility — additional labels (e.g.
//! per-client profile, TLD) will be added in stories 02-003 and 05-001 without
//! changing call sites that only need the basic shape.
//!
//! [`metrics`]: https://docs.rs/metrics/latest/metrics/

use metrics::{counter, gauge};

// ---------------------------------------------------------------------------
// Query rate
// ---------------------------------------------------------------------------

/// Record a DNS query. Increments `dnshub_queries_total` with labels
/// `client` (the client tag / profile name, never a raw IP) and `qtype`
/// (e.g. `"A"`, `"AAAA"`, `"MX"`).
///
/// Label cardinality is bounded by using `client_tag` rather than raw client
/// IPs (PRD section 4.6).
pub fn record_query(client_tag: &str, qtype: &str) {
    counter!("dnshub_queries_total", "client" => client_tag.to_string(), "qtype" => qtype.to_string()).increment(1);
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
}
