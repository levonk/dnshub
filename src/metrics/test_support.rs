//! Shared test infrastructure for the metrics module.
//!
//! The `metrics` facade routes all `counter!`/`gauge!` calls to a process-wide
//! global recorder that can only be installed once. Tests across the
//! `recorder` and `counters` submodules share a single recorder via
//! [`handle`], which is installed lazily on first use and reused thereafter.

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();

/// Install the global Prometheus recorder once for the test process and return
/// a cloneable handle whose `render` method produces the current scrape output.
///
/// The recorder is installed without an HTTP listener (see
/// [`PrometheusBuilder::install_recorder`]) so tests do not bind a real port;
/// the rendered text is identical to what the scrape endpoint serves.
pub(crate) fn handle() -> PrometheusHandle {
    HANDLE
        .get_or_init(|| {
            PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install Prometheus recorder for tests")
        })
        .clone()
}
