//! Blocklist endpoints per PRD lines 1349-1350.
//!
//! - `GET  /api/v1/blocklists/sources` — blocklist source health
//! - `POST /api/v1/blocklists/refresh` — trigger manual refresh

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::error::ApiError;
use crate::api::state::AppState;

/// Source health entry in the API response.
#[derive(Debug, Serialize)]
pub struct SourceHealthDto {
    pub name: String,
    pub healthy: bool,
    pub circuit_state: String,
    pub consecutive_failures: u32,
    pub url: String,
}

/// GET /api/v1/blocklists/sources — blocklist source health.
pub async fn list_blocklist_sources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SourceHealthDto>>, ApiError> {
    let config = state
        .blocklist_config
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("blocklist config not configured".into()))?;

    let health = state.source_health.as_ref();

    let sources: Vec<SourceHealthDto> = config
        .sources
        .iter()
        .enumerate()
        .map(|(idx, src)| {
            let (healthy, circuit_state, consecutive_failures) = match health {
                Some(h) => {
                    if let Some(tracker) = h.get(idx) {
                        (
                            tracker.is_healthy(),
                            format!("{:?}", tracker.state()),
                            tracker.consecutive_failures(),
                        )
                    } else {
                        (true, "unknown".to_string(), 0)
                    }
                }
                None => (true, "unknown".to_string(), 0),
            };
            SourceHealthDto {
                name: src.name.clone(),
                url: src.url.clone(),
                healthy,
                circuit_state,
                consecutive_failures,
            }
        })
        .collect();

    Ok(Json(sources))
}

/// Response body for POST /api/v1/blocklists/refresh.
#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub message: String,
    pub triggered: bool,
}

/// POST /api/v1/blocklists/refresh — trigger manual blocklist refresh.
pub async fn trigger_blocklist_refresh(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RefreshResponse>, ApiError> {
    let notify = state
        .blocklist_refresh
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("blocklist daemon not configured".into()))?;

    notify.notify_one();
    tracing::info!("blocklist refresh triggered via API");

    Ok(Json(RefreshResponse {
        message: "blocklist refresh triggered".to_string(),
        triggered: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::config::{BlocklistsConfig, SourceConfig};
    use crate::blocklist::health::SourceHealthRegistry;
    use crate::config::{ConfigStore, DnshubConfig};
    use axum::extract::State;
    use std::time::Duration;
    use tokio::sync::Notify;

    fn blocklist_config() -> Arc<BlocklistsConfig> {
        Arc::new(BlocklistsConfig {
            sources: vec![SourceConfig {
                name: "test-source".to_string(),
                url: "https://example.com/list.txt".to_string(),
                format: crate::blocklist::config::Format::Domains,
                categories: vec!["ads".to_string()],
                refresh_hours: Some(24),
                refresh_minutes: None,
            }],
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn list_sources() {
        let cfg = blocklist_config();
        let health = Arc::new(SourceHealthRegistry::for_sources(
            vec!["test-source".to_string()],
            5,
            Duration::from_secs(60),
        ));
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .blocklist_config(cfg)
                .source_health(health)
                .build(),
        );
        let Json(sources) = list_blocklist_sources(State(state)).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].name, "test-source");
        assert!(sources[0].healthy);
    }

    #[tokio::test]
    async fn refresh_triggers_notify() {
        let notify = Arc::new(Notify::new());
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .blocklist_refresh(notify.clone())
                .build(),
        );

        // Race the notify against the handler.
        let notify_fut = notify.notified();
        let Json(result) = trigger_blocklist_refresh(State(state)).await.unwrap();
        assert!(result.triggered);

        // The notify should have been signalled.
        tokio::time::timeout(Duration::from_millis(100), notify_fut)
            .await
            .expect("notify was signalled");
    }

    #[tokio::test]
    async fn sources_unavailable_when_not_configured() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let result = list_blocklist_sources(State(state)).await;
        assert!(matches!(result, Err(ApiError::Unavailable(_))));
    }

    #[tokio::test]
    async fn refresh_unavailable_when_not_configured() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let result = trigger_blocklist_refresh(State(state)).await;
        assert!(matches!(result, Err(ApiError::Unavailable(_))));
    }
}
