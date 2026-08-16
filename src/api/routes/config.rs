//! Config endpoints: GET / PUT `/api/v1/config`.
//!
//! - `GET /api/v1/config` returns the active [`DnshubConfig`] as JSON.
//! - `PUT /api/v1/config` accepts a TOML string body, parses and validates
//!   it, writes it back to the config file (if `config_path` is set), and
//!   atomically swaps the active config via [`ConfigStore`].

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::state::AppState;
use crate::config::DnshubConfig;

/// GET /api/v1/config — return the active configuration as JSON.
pub async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DnshubConfig>, ApiError> {
    let config = state.config_store.load_full();
    Ok(Json((*config).clone()))
}

/// Request body for PUT /api/v1/config — a raw TOML string.
#[derive(Debug, Deserialize)]
pub struct UpdateConfigRequest {
    /// TOML-encoded configuration to validate and apply.
    pub toml: String,
}

/// Response body for PUT /api/v1/config.
#[derive(Debug, Serialize)]
pub struct UpdateConfigResponse {
    pub message: String,
    pub reloaded: bool,
}

/// PUT /api/v1/config — validate, write, and hot-reload the config.
///
/// The request body is `{"toml": "..."}`. The TOML is parsed into a
/// [`DnshubConfig`], validated, and if `config_path` is set, written to
/// disk. The active config is then atomically swapped via
/// [`ConfigStore::swap`].
pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateConfigRequest>,
) -> Result<Json<UpdateConfigResponse>, ApiError> {
    // Parse the TOML into a DnshubConfig.
    let new_config: DnshubConfig = toml::from_str(&req.toml)
        .map_err(|e| ApiError::BadRequest(format!("TOML parse error: {e}")))?;

    // Validate the parsed config.
    new_config.validate()?;

    // Write to disk if a config path is configured.
    if let Some(path) = &state.config_path {
        write_config_toml(path, &new_config)?;
        tracing::info!(path = ?path, "config written to disk via API");
    }

    // Atomically swap the active config.
    state.config_store.swap(new_config);

    Ok(Json(UpdateConfigResponse {
        message: "configuration updated and reloaded".to_string(),
        reloaded: true,
    }))
}

/// Write the TOML representation of the config to disk.
fn write_config_toml(path: &std::path::Path, config: &DnshubConfig) -> Result<(), ApiError> {
    let toml_str = toml::to_string(config)
        .map_err(|e| ApiError::Internal(format!("TOML serialize error: {e}")))?;
    std::fs::write(path, toml_str)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, DnshubConfig};
    use axum::extract::State;

    fn test_state(config: DnshubConfig) -> Arc<AppState> {
        Arc::new(AppState::builder(Arc::new(ConfigStore::new(config))).build())
    }

    #[tokio::test]
    async fn get_config_returns_json() {
        let config = DnshubConfig::defaults();
        let state = test_state(config.clone());
        let Json(result) = get_config(State(state)).await.unwrap();
        // The returned config should have a server section.
        assert!(!result.server.listen.is_empty());
    }

    #[tokio::test]
    async fn put_config_invalid_toml_returns_400() {
        let state = test_state(DnshubConfig::defaults());
        let req = UpdateConfigRequest {
            toml: "not valid toml = =".to_string(),
        };
        let result = put_config(State(state), Json(req)).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn put_config_valid_toml_swaps_config() {
        let state = test_state(DnshubConfig::defaults());
        let toml = toml::to_string(&DnshubConfig::defaults()).unwrap();
        let req = UpdateConfigRequest { toml };
        let result = put_config(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());
        let Json(resp) = result.unwrap();
        assert!(resp.reloaded);
        // Config store should still be loadable.
        let _ = state.config_store.load_full();
    }

    #[tokio::test]
    async fn put_config_invalid_validation_returns_error() {
        let state = test_state(DnshubConfig::defaults());
        // An empty config with no upstreams should fail validation.
        let req = UpdateConfigRequest {
            toml: "".to_string(),
        };
        let result = put_config(State(state), Json(req)).await;
        assert!(result.is_err());
    }
}
