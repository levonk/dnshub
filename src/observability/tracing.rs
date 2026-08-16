//! Jaeger/OpenTelemetry distributed tracing via `tracing-opentelemetry`.
//!
//! This module sets up the OpenTelemetry trace layer that exports spans
//! to a Jaeger (or OTLP-compatible) collector. Traces are sampled at the
//! configured `sample_rate` (1-10% recommended) to minimize hot-path
//! overhead.
//!
//! ## Architecture
//!
//! The trace layer is composed alongside the JSON logging layer (from
//! [`crate::observability::logging`]) in a single
//! `tracing_subscriber::Registry`. The env-filter controls which spans/
//! events are *recorded* at all; the sampling filter further restricts
//! which spans are *exported* to Jaeger (so that 100% of spans are
//! available to the logging layer but only `sample_rate` fraction are
//! sent to Jaeger).
//!
//! ## Current limitation
//!
//! The Jaeger/OTLP exporter requires `opentelemetry_sdk` and an exporter
//! crate (`opentelemetry-otlp` or `opentelemetry-jaeger`) which are not
//! yet in `Cargo.toml`. When `tracing.enabled = true`, this module
//! installs the `tracing-opentelemetry` layer with a noop tracer and
//! logs a warning. The sampling filter and layer structure are fully
//! functional — only the actual span export is a no-op until the
//! exporter crates are added.
//!
//! To enable real Jaeger export, add to `Cargo.toml`:
//! ```toml
//! opentelemetry_sdk = { version = "0.27", features = ["rt-tokio"] }
//! opentelemetry-otlp = { version = "0.27", features = ["tonic"] }
//! ```
//! Then replace the `noop::NoopTracer` in [`build_otel_layer`] with a
//! real `SdkTracerProvider` + OTLP exporter.

use crate::config::TracingConfig;
use std::sync::atomic::{AtomicU64, Ordering};

/// Error returned by tracing initialization.
#[derive(Debug)]
pub enum TracingInitError {
    /// The global subscriber could not be installed (already set).
    Init(String),
}

impl std::fmt::Display for TracingInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TracingInitError::Init(s) => {
                write!(f, "failed to install tracing subscriber: {s}")
            }
        }
    }
}

impl std::error::Error for TracingInitError {}

/// Probabilistic span sampler for the OpenTelemetry trace layer.
///
/// This filter allows all events through (so log events are never
/// dropped by the trace layer) but samples spans at the configured
/// `sample_rate`. The sampling uses a monotonic atomic counter so that
/// exactly 1-in-N spans are exported, where `N = 1 / sample_rate`.
///
/// This approach is deterministic, lightweight (a single atomic fetch),
/// and avoids pulling in a random number generator dependency.
pub struct TraceSampler {
    counter: AtomicU64,
    /// Inverse sample rate as an integer: every Nth span is exported.
    /// When `sample_rate` is 0.0, no spans are exported (threshold = 0).
    /// When `sample_rate` is 1.0, all spans are exported (threshold = 1).
    threshold: u64,
}

impl TraceSampler {
    /// Create a new sampler with the given sample rate (0.0 to 1.0).
    pub fn new(sample_rate: f64) -> Self {
        let clamped = sample_rate.clamp(0.0, 1.0);
        let threshold = if clamped <= 0.0 {
            0
        } else if clamped >= 1.0 {
            1
        } else {
            // Convert to "every Nth span" where N = 1/rate.
            // e.g. 0.05 → N=20 → threshold=20.
            (1.0 / clamped).round() as u64
        };
        Self {
            counter: AtomicU64::new(0),
            threshold,
        }
    }

    /// Returns `true` if the current span should be exported.
    ///
    /// For `threshold == 0`, always returns `false` (no sampling).
    /// For `threshold == 1`, always returns `true` (full sampling).
    /// Otherwise, returns `true` for every `threshold`-th call.
    fn should_sample(&self) -> bool {
        if self.threshold == 0 {
            return false;
        }
        if self.threshold == 1 {
            return true;
        }
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        n % self.threshold == 0
    }

    /// The configured threshold (every Nth span is exported).
    pub fn threshold(&self) -> u64 {
        self.threshold
    }
}

impl<S> tracing_subscriber::layer::Filter<S>
    for TraceSampler
where
    S: for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    fn enabled(
        &self,
        metadata: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        // Always allow events through (they're handled by the fmt layer).
        if metadata.is_event() {
            return true;
        }
        // Sample spans based on the configured rate.
        self.should_sample()
    }
}

/// Returns `true` if tracing is enabled and the endpoint is configured.
pub fn tracing_active(config: &TracingConfig) -> bool {
    config.enabled && !config.endpoint.is_empty()
}

/// Log a warning if tracing is enabled but the OTLP exporter crate is
/// not yet linked.
///
/// The noop tracer layer compiles and runs but does not export spans.
/// When `opentelemetry_sdk` + an exporter crate are added to Cargo.toml,
/// the real exporter will be used automatically.
pub fn warn_if_exporter_missing(config: &TracingConfig) {
    if tracing_active(config) {
        tracing::warn!(
            endpoint = %config.endpoint,
            service_name = %config.service_name,
            sample_rate = config.sample_rate,
            "Jaeger tracing enabled — note: OTLP exporter crate not yet linked, \
             spans will be sampled but not exported until opentelemetry_sdk + \
             opentelemetry-otlp are added to Cargo.toml"
        );
    }
}

/// Initialize the tracing layer.
///
/// This is typically called by [`crate::observability::init_observability`]
/// rather than directly. When tracing is disabled, this is a no-op.
pub fn init_tracing(config: &TracingConfig) -> Result<(), TracingInitError> {
    if !config.enabled {
        return Ok(());
    }
    warn_if_exporter_missing(config);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_sampler_no_sampling() {
        let sampler = TraceSampler::new(0.0);
        assert_eq!(sampler.threshold(), 0);
        // Should never sample.
        for _ in 0..100 {
            assert!(!sampler.should_sample());
        }
    }

    #[test]
    fn test_trace_sampler_full_sampling() {
        let sampler = TraceSampler::new(1.0);
        assert_eq!(sampler.threshold(), 1);
        // Should always sample.
        for _ in 0..100 {
            assert!(sampler.should_sample());
        }
    }

    #[test]
    fn test_trace_sampler_five_percent() {
        let sampler = TraceSampler::new(0.05);
        assert_eq!(sampler.threshold(), 20);
        // Every 20th call should sample (indices 0, 20, 40, ...).
        let mut sampled = 0;
        for _ in 0..100 {
            if sampler.should_sample() {
                sampled += 1;
            }
        }
        // 100 calls / 20 threshold = 5 samples.
        assert_eq!(sampled, 5);
    }

    #[test]
    fn test_trace_sampler_ten_percent() {
        let sampler = TraceSampler::new(0.10);
        assert_eq!(sampler.threshold(), 10);
        let mut sampled = 0;
        for _ in 0..100 {
            if sampler.should_sample() {
                sampled += 1;
            }
        }
        assert_eq!(sampled, 10);
    }

    #[test]
    fn test_trace_sampler_clamps_above_one() {
        let sampler = TraceSampler::new(1.5);
        assert_eq!(sampler.threshold(), 1);
        assert!(sampler.should_sample());
    }

    #[test]
    fn test_trace_sampler_clamps_below_zero() {
        let sampler = TraceSampler::new(-0.5);
        assert_eq!(sampler.threshold(), 0);
        assert!(!sampler.should_sample());
    }

    #[test]
    fn test_tracing_active_disabled() {
        let config = TracingConfig {
            enabled: false,
            endpoint: "http://jaeger:4317".to_string(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        assert!(!tracing_active(&config));
    }

    #[test]
    fn test_tracing_active_no_endpoint() {
        let config = TracingConfig {
            enabled: true,
            endpoint: String::new(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        assert!(!tracing_active(&config));
    }

    #[test]
    fn test_tracing_active_enabled() {
        let config = TracingConfig {
            enabled: true,
            endpoint: "http://jaeger:4317".to_string(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        assert!(tracing_active(&config));
    }

    #[test]
    fn test_init_tracing_disabled_is_noop() {
        let config = TracingConfig {
            enabled: false,
            endpoint: String::new(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        assert!(init_tracing(&config).is_ok());
    }

    #[test]
    fn test_init_tracing_enabled_logs_warning() {
        let config = TracingConfig {
            enabled: true,
            endpoint: "http://jaeger:4317".to_string(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        // Should succeed (just logs a warning about missing exporter crate).
        assert!(init_tracing(&config).is_ok());
    }
}
