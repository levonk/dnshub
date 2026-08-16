//! Prometheus recorder installation and HTTP scrape endpoint.
//!
//! [`init`] installs the global Prometheus recorder and starts an HTTP
//! listener that serves Prometheus text-format output on the configured
//! address (default `0.0.0.0:9090`). The listener responds to `GET` on any
//! request path, so the configured `path` is informational.
//!
//! [`init_recorder`] installs only the recorder (no HTTP server) and returns a
//! [`PrometheusHandle`] whose [`render`][PrometheusHandle::render] method
//! produces the exact text the scrape endpoint would serve. This is used by
//! the unit tests and by callers that want to expose metrics through their own
//! HTTP server.

use std::net::SocketAddr;

use metrics_exporter_prometheus::{BuildError, PrometheusBuilder};
pub use metrics_exporter_prometheus::PrometheusHandle;

use super::config::MetricsConfig;

/// Error returned by [`init`] / [`init_recorder`] when the recorder cannot be
/// installed or the listen address is invalid.
#[derive(Debug)]
pub enum MetricsError {
    /// The configured `listen` string is not a valid `SocketAddr`.
    InvalidListenAddress(String),

    /// The Prometheus builder failed to install the recorder/exporter.
    Builder(#[allow(dead_code)] BuildError),
}

impl std::fmt::Display for MetricsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidListenAddress(msg) => {
                write!(f, "invalid metrics listen address: {msg}")
            }
            Self::Builder(err) => write!(f, "prometheus builder error: {err}"),
        }
    }
}

impl std::error::Error for MetricsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Builder(err) => Some(err),
            Self::InvalidListenAddress(_) => None,
        }
    }
}

impl From<BuildError> for MetricsError {
    fn from(err: BuildError) -> Self {
        Self::Builder(err)
    }
}

/// Install the Prometheus recorder globally and start the HTTP scrape
/// endpoint on `config.listen`.
///
/// This must be called once at startup, before any helper in
/// [`super::counters`] is invoked. The HTTP listener serves Prometheus
/// text-format output (`text/plain; version=0.0.4`) on every request path.
///
/// # Errors
///
/// Returns [`MetricsError::InvalidListenAddress`] if `config.listen` cannot be
/// parsed as a [`SocketAddr`], or [`MetricsError::Builder`] if the recorder or
/// exporter cannot be installed (e.g. a global recorder is already set).
pub fn init(config: &MetricsConfig) -> Result<(), MetricsError> {
    let addr: SocketAddr = config
        .listen
        .parse()
        .map_err(|e: std::net::AddrParseError| MetricsError::InvalidListenAddress(e.to_string()))?;
    PrometheusBuilder::new()
        .with_http_listener(addr)
        .install()?;
    Ok(())
}

/// Install only the global Prometheus recorder (no HTTP server) and return a
/// handle that can render the current scrape output on demand.
///
/// The caller is responsible for periodic upkeep if long-lived; for the basic
/// counter/gauge metrics in this story, upkeep is not required.
///
/// # Errors
///
/// Returns [`MetricsError::Builder`] if a global recorder is already installed.
pub fn init_recorder() -> Result<PrometheusHandle, MetricsError> {
    let handle = PrometheusBuilder::new().install_recorder()?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::test_support::handle;

    #[test]
    fn render_returns_prometheus_text_format() {
        // The shared test recorder is installed lazily on first use.
        let h = handle();
        // Record a marker metric so the render output is non-empty. The
        // recorder only emits metrics that have been touched at least once.
        metrics::counter!("dnshub_render_probe").increment(1);
        let out = h.render();
        assert!(
            out.contains("dnshub_render_probe"),
            "expected render output to contain the probe metric, got:\n{out}"
        );
        // Prometheus text format emits `<metric_name> <value>` lines.
        assert!(
            out.contains("dnshub_render_probe 1"),
            "expected render output to contain the probe value, got:\n{out}"
        );
    }

    #[test]
    fn init_rejects_invalid_listen_address() {
        let cfg = MetricsConfig {
            listen: "not-a-socket-addr".to_string(),
            path: "/metrics".to_string(),
        };
        let res = init(&cfg);
        assert!(
            matches!(res, Err(MetricsError::InvalidListenAddress(_))),
            "expected InvalidListenAddress, got {res:?}"
        );
    }

    #[test]
    fn metrics_error_display_is_informative() {
        let err = MetricsError::InvalidListenAddress("bad addr".to_string());
        let msg = err.to_string();
        assert!(msg.contains("invalid metrics listen address"), "got: {msg}");
        assert!(msg.contains("bad addr"), "got: {msg}");
    }
}
