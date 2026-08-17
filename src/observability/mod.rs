//! Observability infrastructure: structured JSON logging and Jaeger
//! distributed tracing.
//!
//! This module wires together the logging and tracing subsystems into a
//! single `tracing_subscriber` registry. It is the entry point for
//! observability initialization at server startup.
//!
//! ## Sub-modules
//!
//! - [`config`] — unified config view and log field name constants
//! - [`logging`] — JSON logging via `tracing-subscriber` (stdout → Loki)
//! - [`tracing`] — Jaeger trace export via `tracing-opentelemetry`
//!
//! ## Usage
//!
//! ```no_run
//! use dnshub::config::DnshubConfig;
//! use dnshub::observability::init_observability;
//!
//! let config = DnshubConfig::defaults();
//! init_observability(&config).expect("failed to init observability");
//! ```
//!
//! ## Log format
//!
//! JSON logs are emitted to stdout in the format defined by PRD section
//! 4.8 (lines 1253-1273). Promtail or Alloy picks them up and ships to
//! Loki. See [`config::fields`] for the canonical field names.

pub mod config;
pub mod logging;
pub mod tracing;

use crate::config::DnshubConfig;
use crate::observability::config::ObservabilityConfig;
use crate::observability::logging::{build_env_filter, is_json_format, LoggingInitError};
use crate::observability::tracing::{init_tracing, tracing_active, TraceSampler, TracingInitError};
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

/// Error returned by [`init_observability`].
#[derive(Debug)]
pub enum ObservabilityError {
    /// Logging initialization failed (bad level or subscriber already set).
    Logging(LoggingInitError),
    /// Tracing initialization failed.
    Tracing(TracingInitError),
}

impl std::fmt::Display for ObservabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ObservabilityError::Logging(e) => write!(f, "logging init: {e}"),
            ObservabilityError::Tracing(e) => write!(f, "tracing init: {e}"),
        }
    }
}

impl std::error::Error for ObservabilityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ObservabilityError::Logging(e) => Some(e),
            ObservabilityError::Tracing(e) => Some(e),
        }
    }
}

impl From<LoggingInitError> for ObservabilityError {
    fn from(e: LoggingInitError) -> Self {
        ObservabilityError::Logging(e)
    }
}

impl From<TracingInitError> for ObservabilityError {
    fn from(e: TracingInitError) -> Self {
        ObservabilityError::Tracing(e)
    }
}

/// Initialize the observability stack (logging + tracing) as the global
/// default subscriber.
///
/// This composes:
/// 1. An `EnvFilter` from `[logging].level` (or `RUST_LOG` if set).
/// 2. A JSON `fmt` layer writing to stdout (or plain if
///    `[logging].format = "plain"`).
/// 3. An OpenTelemetry trace layer with sampling (if `[tracing].enabled
///    = true`).
///
/// All three are installed as a single `tracing_subscriber::Registry`
/// via `try_init`, which returns an error rather than panicking if the
/// global subscriber is already set.
///
/// # Errors
///
/// Returns [`ObservabilityError::Logging`] if the log level is invalid
/// or the subscriber cannot be installed.
pub fn init_observability(config: &DnshubConfig) -> Result<(), ObservabilityError> {
    let obs = ObservabilityConfig::from_config(config);
    let env_filter = build_env_filter(&obs.logging)?;
    let use_json = is_json_format(&obs.logging);
    let use_tracing = tracing_active(&obs.tracing);

    // Build the real OTLP tracer provider before installing the subscriber.
    // The provider must be created before the tracing-opentelemetry layer
    // is installed so that the layer picks up the global tracer provider.
    // We leak the provider intentionally — it must outlive all traced spans
    // for the entire process lifetime. On normal process exit the OS
    // reclaims the memory; a more sophisticated caller could hold it in
    // main() and call shutdown() for a clean flush.
    if use_tracing {
        let provider = init_tracing(&obs.tracing)?;
        if provider.is_some() {
            // Hold the provider alive for the process lifetime. The
            // batch exporter runs on a background Tokio task.
            // We intentionally leak it — it must not be dropped while
            // spans are still being recorded.
            std::mem::forget(provider);
        }
    }

    // Build the subscriber. We use four branches (json/plain × tracing/no-tracing)
    // because each composition produces a different concrete type — the layers
    // cannot be boxed without losing the `Layer<Layered<...>>` impl.
    let init_result = match (use_json, use_tracing) {
        (true, true) => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_target(true))
            .with(
                tracing_opentelemetry::layer()
                    .with_filter(TraceSampler::new(obs.clamped_sample_rate())),
            )
            .try_init(),
        (true, false) => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_target(true))
            .try_init(),
        (false, true) => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(true))
            .with(
                tracing_opentelemetry::layer()
                    .with_filter(TraceSampler::new(obs.clamped_sample_rate())),
            )
            .try_init(),
        (false, false) => tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(true))
            .try_init(),
    };

    init_result.map_err(|e| {
        ObservabilityError::Logging(LoggingInitError::Init(e.to_string()))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DnshubConfig, LoggingConfig, TracingConfig};

    #[test]
    fn test_init_observability_with_defaults() {
        // The global subscriber may already be set by another test or by
        // main(). try_init returns an error in that case, which we accept.
        let config = DnshubConfig::defaults();
        let result = init_observability(&config);
        // Either it succeeds (first call) or fails with Init error (already set).
        match result {
            Ok(()) => {}
            Err(ObservabilityError::Logging(LoggingInitError::Init(_))) => {}
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn test_init_observability_invalid_level() {
        // Clear RUST_LOG so the config level is used (not an env override).
        std::env::remove_var("RUST_LOG");
        let mut config = DnshubConfig::defaults();
        config.logging = LoggingConfig {
            level: "verbose".to_string(),
            format: "json".to_string(),
        };
        let result = init_observability(&config);
        // If RUST_LOG was set by a parallel test, the filter may succeed
        // and try_init may fail with Init instead. Accept both.
        match result {
            Err(ObservabilityError::Logging(LoggingInitError::InvalidLevel(_))) => {}
            Err(ObservabilityError::Logging(LoggingInitError::Init(_))) => {}
            other => panic!("expected error, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_init_observability_with_tracing_enabled() {
        let mut config = DnshubConfig::defaults();
        config.tracing = TracingConfig {
            enabled: true,
            endpoint: "http://jaeger:4317".to_string(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        let result = init_observability(&config);
        // The OTLP provider builds lazily (tonic channel doesn't connect
        // immediately). Accept success, "already initialized" error, or
        // a tracing init error (if the global tracer provider was already
        // set by a parallel test).
        match result {
            Ok(()) => {}
            Err(ObservabilityError::Logging(LoggingInitError::Init(_))) => {}
            Err(ObservabilityError::Tracing(_)) => {}
            Err(e) => panic!("unexpected error: {e}"),
        }
    }

    #[test]
    fn test_observability_error_display() {
        let err = ObservabilityError::Logging(LoggingInitError::InvalidLevel(
            "bad".to_string(),
        ));
        assert!(err.to_string().contains("logging init"));
        assert!(err.to_string().contains("invalid log level"));
    }
}
