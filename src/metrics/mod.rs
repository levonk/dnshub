//! Prometheus metrics collection and export.
//!
//! This module is the observability foundation for dnshub. It installs a
//! Prometheus recorder (via the allocation-free [`metrics`] facade) and starts
//! an HTTP scrape endpoint on `:9090/metrics` by default. Helper functions in
//! [`counters`] record the basic DNS query, cache, blocklist, and error
//! metrics defined in PRD section 4.6.
//!
//! ## Usage
//!
//! Call [`init`] once at startup (before any metric helper) to install the
//! global recorder and start the scrape endpoint:
//!
//! ```no_run
//! use dnshub::config::MetricsConfig;
//! use dnshub::metrics;
//!
//! let cfg = MetricsConfig {
//!     listen: "0.0.0.0:9090".to_string(),
//!     path: "/metrics".to_string(),
//! };
//! metrics::init(&cfg).expect("failed to start metrics endpoint");
//! ```
//!
//! The DNS handler chain (wired in Phase 02) then calls the helpers in
//! [`counters`] on the hot path, e.g. [`counters::record_query`].
//!
//! Metric names follow Prometheus conventions: `snake_case` with the `dnshub_`
//! prefix. Label cardinality is bounded by using `client_tag` (profile name or
//! hostname) rather than raw client IPs (PRD section 4.6).
//!
//! [`metrics`]: https://docs.rs/metrics/latest/metrics/

pub mod config;
pub mod counters;
pub mod recorder;

pub use config::MetricsConfig;
pub use recorder::{init, init_recorder, MetricsError, PrometheusHandle};

#[cfg(test)]
mod test_support;
