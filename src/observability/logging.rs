//! Structured JSON logging via `tracing-subscriber`.
//!
//! This module sets up the `tracing` subscriber with a `fmt` layer that
//! emits structured JSON to stdout. Promtail or Alloy picks up the JSON
//! lines and ships them to Loki for long-term storage and Grafana
//! correlation.
//!
//! ## Log format
//!
//! The JSON output matches the PRD section 4.8 schema (lines 1253-1273):
//!
//! ```json
//! {
//!   "timestamp": "2026-08-10T12:34:56.789Z",
//!   "level": "info",
//!   "target": "dnshub::query_log",
//!   "fields": { ... }
//! }
//! ```
//!
//! The `tracing-subscriber` JSON formatter produces this shape natively:
//! `timestamp`, `level`, `target`, and `fields` are emitted automatically.
//!
//! ## Log level
//!
//! The level is configurable via `[logging].level` in `dnshub.toml` and
//! can be overridden at runtime with the `RUST_LOG` environment variable
//! (which takes precedence when set).

use crate::config::LoggingConfig;
use crate::observability::config::parse_log_level;
use tracing_subscriber::filter::EnvFilter;

/// Error returned by logging initialization.
#[derive(Debug)]
pub enum LoggingInitError {
    /// The configured log level string was not recognized.
    InvalidLevel(String),
    /// The global subscriber could not be installed (already set).
    Init(String),
}

impl std::fmt::Display for LoggingInitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoggingInitError::InvalidLevel(s) => {
                write!(f, "invalid log level: {s}")
            }
            LoggingInitError::Init(s) => {
                write!(f, "failed to install logging subscriber: {s}")
            }
        }
    }
}

impl std::error::Error for LoggingInitError {}

/// Build the env-filter directive string from the logging config.
///
/// The `RUST_LOG` environment variable takes precedence when set.
/// Otherwise, the configured level is used as the global default.
pub fn build_env_filter(config: &LoggingConfig) -> Result<EnvFilter, LoggingInitError> {
    // RUST_LOG always wins if set.
    if let Ok(rust_log) = std::env::var("RUST_LOG") {
        if !rust_log.is_empty() {
            return EnvFilter::try_new(rust_log)
                .map_err(|e| LoggingInitError::InvalidLevel(e.to_string()));
        }
    }

    // Validate the configured level.
    let level = parse_log_level(&config.level).ok_or_else(|| {
        LoggingInitError::InvalidLevel(format!(
            "'{}' is not one of trace/debug/info/warn/error",
            config.level
        ))
    })?;

    Ok(EnvFilter::new(level.to_string().to_lowercase()))
}

/// Returns `true` if the logging config requests JSON format.
///
/// Any value other than `"plain"` defaults to JSON (the PRD default).
pub fn is_json_format(config: &LoggingConfig) -> bool {
    config.format != "plain"
}

/// Initialize the logging subscriber as the global default.
///
/// This installs a `tracing_subscriber::registry()` with the env-filter
/// and the JSON (or plain) fmt layer. If the global subscriber is already
/// set, this returns an error rather than panicking.
pub fn init_logging(config: &LoggingConfig) -> Result<(), LoggingInitError> {
    use tracing_subscriber::fmt;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let env_filter = build_env_filter(config)?;

    if is_json_format(config) {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().json().with_target(true))
            .try_init()
            .map_err(|e| LoggingInitError::Init(e.to_string()))?;
    } else {
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt::layer().with_target(true))
            .try_init()
            .map_err(|e| LoggingInitError::Init(e.to_string()))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_env_filter_from_config() {
        let config = LoggingConfig {
            level: "warn".to_string(),
            format: "json".to_string(),
        };
        // Clear RUST_LOG so the config level is used.
        std::env::remove_var("RUST_LOG");
        let filter = build_env_filter(&config).expect("should build filter");
        // The filter should accept warn-level events.
        assert!(filter.max_level_hint().is_some());
    }

    #[test]
    fn test_build_env_filter_invalid_level() {
        let config = LoggingConfig {
            level: "verbose".to_string(),
            format: "json".to_string(),
        };
        std::env::remove_var("RUST_LOG");
        let result = build_env_filter(&config);
        assert!(result.is_err());
        match result {
            Err(LoggingInitError::InvalidLevel(_)) => {}
            _ => panic!("expected InvalidLevel error"),
        }
    }

    #[test]
    fn test_build_env_filter_rust_log_override() {
        std::env::set_var("RUST_LOG", "debug");
        let config = LoggingConfig {
            level: "error".to_string(),
            format: "json".to_string(),
        };
        let filter = build_env_filter(&config).expect("should build filter");
        // RUST_LOG=debug should override the config level=error.
        // The max level hint should be DEBUG, not ERROR.
        assert_eq!(
            filter.max_level_hint(),
            Some(tracing_subscriber::filter::LevelFilter::DEBUG)
        );
        std::env::remove_var("RUST_LOG");
    }

    #[test]
    fn test_is_json_format() {
        assert!(is_json_format(&LoggingConfig {
            level: "info".to_string(),
            format: "json".to_string(),
        }));
        assert!(!is_json_format(&LoggingConfig {
            level: "info".to_string(),
            format: "plain".to_string(),
        }));
        // Unknown formats default to JSON.
        assert!(is_json_format(&LoggingConfig {
            level: "info".to_string(),
            format: "xml".to_string(),
        }));
    }
}
