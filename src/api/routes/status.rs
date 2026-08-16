//! Status endpoint: GET `/api/v1/status`.
//!
//! Returns service health including uptime, version, upstream tier
//! configuration, and (if available) Prometheus metrics summary.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::state::AppState;
use crate::config::UpstreamConfig;

/// Response body for GET /api/v1/status.
#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub status: String,
    pub version: String,
    pub uptime_secs: u64,
    pub upstreams: Vec<UpstreamStatus>,
    pub dhcp_v4_enabled: bool,
    pub dhcp_v6_enabled: bool,
    pub frontend_enabled: bool,
    pub query_log_enabled: bool,
    pub blocklist_sources: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<String>,
}

/// Upstream tier status.
#[derive(Debug, Serialize)]
pub struct UpstreamStatus {
    pub name: String,
    pub address: String,
    pub tier: u32,
    pub protocol: String,
}

/// GET /api/v1/status — return service health and status.
pub async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Json<StatusResponse> {
    let config = state.config_store.load_full();
    let uptime = state.start_time.elapsed();

    let upstreams: Vec<UpstreamStatus> = config
        .upstreams
        .iter()
        .map(|u: &UpstreamConfig| UpstreamStatus {
            name: u.name.clone(),
            address: u.address.clone(),
            tier: u.tier,
            protocol: u.protocol.clone(),
        })
        .collect();

    let blocklist_sources = state
        .blocklist_config
        .as_ref()
        .map(|c| c.sources.len())
        .unwrap_or(0);

    let metrics = state
        .metrics_handle
        .as_ref()
        .map(|h| h.render());

    Json(StatusResponse {
        status: "ok".to_string(),
        version: crate::VERSION.to_string(),
        uptime_secs: uptime.as_secs(),
        upstreams,
        dhcp_v4_enabled: config.dhcp.v4.enabled,
        dhcp_v6_enabled: false, // v6 config not wired into DnshubConfig yet
        frontend_enabled: config.frontend.enabled,
        query_log_enabled: config.query_log.enabled,
        blocklist_sources,
        metrics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, DnshubConfig};
    use axum::extract::State;
    use std::time::Duration;

    #[tokio::test]
    async fn status_returns_ok() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let Json(result) = get_status(State(state)).await;
        assert_eq!(result.status, "ok");
        assert!(result.uptime_secs == 0 || result.uptime_secs > 0);
        assert!(!result.upstreams.is_empty());
    }

    #[test]
    fn duration_to_secs() {
        let d = Duration::from_secs(42);
        assert_eq!(d.as_secs(), 42);
    }
}
