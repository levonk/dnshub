//! Observability configuration helpers.
//!
//! This module provides a unified view of the logging and tracing
//! configuration sections ([`LoggingConfig`] and [`TracingConfig`]) so
//! that [`crate::observability`] can consume them without reaching into
//! [`crate::config`] at every call site.
//!
//! It also defines the structured log fields that the DNS handler chain
//! emits per PRD section 4.8 (lines 1253-1273). These field names are the
//! contract between dnshub and Loki/Grafana — changing them breaks
//! dashboards and LogQL queries.

use crate::config::{LoggingConfig, TracingConfig};

/// Unified view of the observability-related config sections.
///
/// Extracted from [`crate::config::DnshubConfig`] so the observability
/// module has a single, focused type to work with.
#[derive(Debug, Clone)]
pub struct ObservabilityConfig {
    /// Structured logging configuration (`[logging]` section).
    pub logging: LoggingConfig,
    /// Jaeger/OpenTelemetry tracing configuration (`[tracing]` section).
    pub tracing: TracingConfig,
}

impl ObservabilityConfig {
    /// Extract the observability sections from a full [`DnshubConfig`].
    pub fn from_config(config: &crate::config::DnshubConfig) -> Self {
        Self {
            logging: config.logging.clone(),
            tracing: config.tracing.clone(),
        }
    }

    /// Returns `true` if Jaeger trace export is enabled and configured.
    pub fn tracing_enabled(&self) -> bool {
        self.tracing.enabled && !self.tracing.endpoint.is_empty()
    }

    /// Effective sample rate clamped to `[0.0, 1.0]`.
    pub fn clamped_sample_rate(&self) -> f64 {
        self.tracing.sample_rate.clamp(0.0, 1.0)
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            logging: LoggingConfig::default(),
            tracing: TracingConfig::default(),
        }
    }
}

/// Canonical structured log field names for DNS query log entries.
///
/// These match the JSON schema in PRD section 4.8 (lines 1253-1273).
/// Every query log event emitted by the DNS handler chain should include
/// these fields so that Loki/Grafana can correlate logs with metrics.
///
/// # Example JSON output
///
/// ```json
/// {
///   "timestamp": "2026-08-10T12:34:56.789Z",
///   "level": "info",
///   "target": "dnshub::query_log",
///   "fields": {
///     "client_ip": "192.168.1.20",
///     "client_name": "kids-tablet",
///     "profile": "kids",
///     "domain": "tiktok.com",
///     "qtype": "A",
///     "response_code": "NXDOMAIN",
///     "blocked": true,
///     "block_category": "social",
///     "block_source": "stevenblack-social",
///     "tier": null,
///     "latency_ms": 0,
///     "cached": false
///   }
/// }
/// ```
pub mod fields {
    /// Client IP address (e.g. `"192.168.1.20"`).
    pub const CLIENT_IP: &str = "client_ip";
    /// Client hostname from DHCP lease table (e.g. `"kids-tablet"`).
    pub const CLIENT_NAME: &str = "client_name";
    /// Policy profile applied (e.g. `"kids"`, `"default"`).
    pub const PROFILE: &str = "profile";
    /// Queried domain name (e.g. `"tiktok.com"`).
    pub const DOMAIN: &str = "domain";
    /// DNS query type (e.g. `"A"`, `"AAAA"`, `"MX"`).
    pub const QTYPE: &str = "qtype";
    /// DNS response code (e.g. `"NOERROR"`, `"NXDOMAIN"`, `"SERVFAIL"`).
    pub const RESPONSE_CODE: &str = "response_code";
    /// Whether the query was blocked by a blocklist rule.
    pub const BLOCKED: &str = "blocked";
    /// Blocklist category if blocked (e.g. `"social"`, `"ads"`).
    pub const BLOCK_CATEGORY: &str = "block_category";
    /// Blocklist source name if blocked (e.g. `"stevenblack-social"`).
    pub const BLOCK_SOURCE: &str = "block_source";
    /// Upstream tier that served the response (1-5, or `null` if cached/blocked).
    pub const TIER: &str = "tier";
    /// Resolution latency in milliseconds.
    pub const LATENCY_MS: &str = "latency_ms";
    /// Whether the response was served from cache.
    pub const CACHED: &str = "cached";
}

/// Parse a log level string into a validated [`tracing::Level`].
///
/// Accepts case-insensitive `"trace"`, `"debug"`, `"info"`, `"warn"`,
/// `"error"`. Returns `None` for unrecognized levels.
pub fn parse_log_level(s: &str) -> Option<tracing::Level> {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Some(tracing::Level::TRACE),
        "debug" => Some(tracing::Level::DEBUG),
        "info" => Some(tracing::Level::INFO),
        "warn" => Some(tracing::Level::WARN),
        "error" => Some(tracing::Level::ERROR),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_observability_config_default() {
        let config = ObservabilityConfig::default();
        assert!(!config.tracing_enabled());
        assert_eq!(config.logging.level, "info");
        assert_eq!(config.logging.format, "json");
        assert!((config.clamped_sample_rate() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_tracing_enabled_requires_endpoint() {
        let config = ObservabilityConfig {
            tracing: TracingConfig {
                enabled: true,
                endpoint: String::new(),
                sample_rate: 0.1,
                service_name: "dnshub".to_string(),
            },
            ..Default::default()
        };
        // enabled but no endpoint → not actually enabled
        assert!(!config.tracing_enabled());
    }

    #[test]
    fn test_tracing_enabled_with_endpoint() {
        let config = ObservabilityConfig {
            tracing: TracingConfig {
                enabled: true,
                endpoint: "http://jaeger:4317".to_string(),
                sample_rate: 0.1,
                service_name: "dnshub".to_string(),
            },
            ..Default::default()
        };
        assert!(config.tracing_enabled());
    }

    #[test]
    fn test_clamped_sample_rate() {
        let mut config = ObservabilityConfig::default();
        config.tracing.sample_rate = 1.5;
        assert!((config.clamped_sample_rate() - 1.0).abs() < f64::EPSILON);
        config.tracing.sample_rate = -0.5;
        assert!((config.clamped_sample_rate() - 0.0).abs() < f64::EPSILON);
        config.tracing.sample_rate = 0.05;
        assert!((config.clamped_sample_rate() - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_parse_log_level_valid() {
        assert_eq!(parse_log_level("info"), Some(tracing::Level::INFO));
        assert_eq!(parse_log_level("DEBUG"), Some(tracing::Level::DEBUG));
        assert_eq!(parse_log_level("Warn"), Some(tracing::Level::WARN));
        assert_eq!(parse_log_level("error"), Some(tracing::Level::ERROR));
        assert_eq!(parse_log_level("trace"), Some(tracing::Level::TRACE));
    }

    #[test]
    fn test_parse_log_level_invalid() {
        assert_eq!(parse_log_level("verbose"), None);
        assert_eq!(parse_log_level(""), None);
        assert_eq!(parse_log_level("INF"), None);
    }

    #[test]
    fn test_field_names_match_prd() {
        // Ensure field names match the PRD JSON schema (lines 1253-1273).
        assert_eq!(fields::CLIENT_IP, "client_ip");
        assert_eq!(fields::CLIENT_NAME, "client_name");
        assert_eq!(fields::PROFILE, "profile");
        assert_eq!(fields::DOMAIN, "domain");
        assert_eq!(fields::QTYPE, "qtype");
        assert_eq!(fields::RESPONSE_CODE, "response_code");
        assert_eq!(fields::BLOCKED, "blocked");
        assert_eq!(fields::BLOCK_CATEGORY, "block_category");
        assert_eq!(fields::BLOCK_SOURCE, "block_source");
        assert_eq!(fields::TIER, "tier");
        assert_eq!(fields::LATENCY_MS, "latency_ms");
        assert_eq!(fields::CACHED, "cached");
    }

    #[test]
    fn test_from_config() {
        let dnshub_config = crate::config::DnshubConfig::defaults();
        let obs = ObservabilityConfig::from_config(&dnshub_config);
        assert_eq!(obs.logging.format, "json");
        assert_eq!(obs.logging.level, "info");
        assert!(!obs.tracing_enabled());
    }
}
