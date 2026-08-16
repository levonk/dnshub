//! Configuration for the `[metrics]` section.
//!
//! The serde struct for the `[metrics]` TOML section lives in
//! [`crate::config::MetricsConfig`] so that the top-level [`crate::config::DnshubConfig`]
//! can deserialize it inline. This module re-exports it as
//! [`MetricsConfig`] so callers of [`super::init`] can refer to it without
//! pulling in the whole config module.
//!
//! Defaults (matching PRD section 4.6):
//!
//! | Field   | Default          | Meaning                              |
//! |---------|------------------|--------------------------------------|
//! | `listen`| `0.0.0.0:9090`   | scrape endpoint bind address         |
//! | `path`  | `/metrics`       | (informational; exporter serves any) |
//!
//! [`crate::config::DnshubConfig`]: crate::config::DnshubConfig

pub use crate::config::MetricsConfig;
