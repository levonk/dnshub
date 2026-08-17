//! Jaeger/OpenTelemetry distributed tracing via `tracing-opentelemetry`.
//!
//! This module sets up the OpenTelemetry trace layer that exports spans
//! to a Jaeger (or OTLP-compatible) collector via the OTLP gRPC protocol.
//! Traces are sampled at the configured `sample_rate` (1-10% recommended)
//! to minimize hot-path overhead.
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
//! When `tracing.enabled = true` and `tracing.endpoint` is set, a real
//! `SdkTracerProvider` with an OTLP gRPC exporter is created and linked
//! to the `tracing-opentelemetry` layer. Spans matching the sampler are
//! exported to the configured OTLP collector (Jaeger, Tempo, etc.).

use crate::config::TracingConfig;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::trace::SdkTracerProvider;
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

/// Build a real `SdkTracerProvider` with an OTLP gRPC exporter pointing
/// at the configured endpoint. The provider is installed as the global
/// tracer provider and the `tracing-opentelemetry` layer is configured
/// to use it.
///
/// Returns the `SdkTracerProvider` so the caller can hold it for the
/// lifetime of the process (the provider must outlive all traced spans).
/// On shutdown, call `provider.shutdown()` to flush pending exports.
pub fn build_otlp_provider(config: &TracingConfig) -> Result<SdkTracerProvider, TracingInitError> {
    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(&config.endpoint)
        .build()
        .map_err(|e| TracingInitError::Init(format!("failed to build OTLP exporter: {e}")))?;

    let provider = SdkTracerProvider::builder()
        .with_batch_exporter(exporter)
        .with_resource(
            opentelemetry_sdk::Resource::builder()
                .with_service_name(config.service_name.clone())
                .build(),
        )
        .build();

    // Install as the global provider so tracing-opentelemetry's layer
    // picks it up.
    opentelemetry::global::set_tracer_provider(provider.clone());

    tracing::info!(
        endpoint = %config.endpoint,
        service_name = %config.service_name,
        sample_rate = config.sample_rate,
        "Jaeger OTLP tracer provider installed — spans will be exported"
    );

    Ok(provider)
}

/// Initialize the tracing layer.
///
/// This is typically called by [`crate::observability::init_observability`]
/// rather than directly. When tracing is disabled, this is a no-op.
pub fn init_tracing(config: &TracingConfig) -> Result<Option<SdkTracerProvider>, TracingInitError> {
    if !config.enabled {
        return Ok(None);
    }
    if !tracing_active(config) {
        tracing::warn!(
            enabled = config.enabled,
            "tracing enabled but no endpoint configured — spans will not be exported"
        );
        return Ok(None);
    }
    let provider = build_otlp_provider(config)?;
    Ok(Some(provider))
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
        // Disabled → returns Ok(None).
        assert!(matches!(init_tracing(&config), Ok(None)));
    }

    #[test]
    fn test_init_tracing_enabled_no_endpoint_returns_none() {
        let config = TracingConfig {
            enabled: true,
            endpoint: String::new(),
            sample_rate: 0.05,
            service_name: "dnshub".to_string(),
        };
        // Enabled but no endpoint → Ok(None) with a warning.
        assert!(matches!(init_tracing(&config), Ok(None)));
    }
}
