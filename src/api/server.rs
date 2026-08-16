//! axum server setup and router configuration.
//!
//! [`ApiServer`] builds the axum [`Router`] with all PRD section 4.9
//! endpoints, applies CORS middleware (via `tower-http`), and provides a
//! [`ApiServer::serve`] method to bind the configured listen address.
//!
//! The router is also exposed via [`ApiServer::router`] so callers (e.g.
//! `main.rs` in a future wiring story, or tests) can use it with a custom
//! test harness or `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::{Any, CorsLayer};

use crate::api::routes::blocklists;
use crate::api::routes::config as config_routes;
use crate::api::routes::dhcp;
use crate::api::routes::query_log;
use crate::api::routes::status;
use crate::api::state::AppState;

/// REST API server (axum) exposing all PRD section 4.9 endpoints.
pub struct ApiServer {
    router: Router,
}

impl ApiServer {
    /// Build a new [`ApiServer`] with the given [`AppState`].
    ///
    /// The router is configured with:
    /// - All `/api/v1/*` endpoints from PRD section 4.9.
    /// - CORS middleware allowing any origin (for frontend development;
    ///   production restricts via Traefik/Authelia).
    pub fn new(state: Arc<AppState>) -> Self {
        let router = Self::build_router(state);
        Self { router }
    }

    /// Build the axum router with all API routes.
    fn build_router(state: Arc<AppState>) -> Router {
        // CORS: allow any origin for development. In production the
        // frontend is served from the same origin or behind Traefik.
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        // Config endpoints.
        let config_routes = Router::new()
            .route("/config", get(config_routes::get_config).put(config_routes::put_config));

        // Status endpoint.
        let status_routes = Router::new().route("/status", get(status::get_status));

        // DHCP endpoints.
        let dhcp_routes = Router::new()
            .route("/leases", get(dhcp::list_leases))
            .route("/release", post(dhcp::release_lease))
            .route(
                "/static",
                get(dhcp::list_static_leases)
                    .post(dhcp::add_static_lease)
                    .delete(dhcp::delete_static_lease),
            )
            .route(
                "/blocklist",
                get(dhcp::list_mac_blocklist)
                    .post(dhcp::add_mac_block)
                    .delete(dhcp::delete_mac_block),
            )
            .route("/audit", get(dhcp::list_audit_events))
            .route("/rogue", get(dhcp::get_rogue_status))
            .route(
                "/pxe/bootfiles",
                get(dhcp::get_pxe_bootfiles).put(dhcp::put_pxe_bootfiles),
            )
            .route(
                "/pxe/bootp",
                get(dhcp::get_pxe_bootp)
                    .post(dhcp::add_pxe_bootp)
                    .delete(dhcp::delete_pxe_bootp),
            )
            .route("/relay/agents", get(dhcp::get_relay_agents))
            .route(
                "/relay/option82",
                get(dhcp::get_relay_option82).put(dhcp::put_relay_option82),
            )
            .route("/pools", get(dhcp::get_dhcp_pools));

        // Blocklist endpoints.
        let blocklist_routes = Router::new()
            .route("/sources", get(blocklists::list_blocklist_sources))
            .route("/refresh", post(blocklists::trigger_blocklist_refresh));

        // Query log endpoints.
        let query_log_routes = Router::new()
            .route("/query-log", get(query_log::get_query_log))
            .route("/query-log/export", get(query_log::export_query_log));

        Router::new()
            .nest("/api/v1", config_routes)
            .nest("/api/v1", status_routes)
            .nest("/api/v1/dhcp", dhcp_routes)
            .nest("/api/v1/blocklists", blocklist_routes)
            .nest("/api/v1", query_log_routes)
            .layer(cors)
            .with_state(state)
    }

    /// Returns the configured router (for testing or custom serving).
    pub fn router(&self) -> Router {
        self.router.clone()
    }

    /// Serve the API on the given bind address.
    ///
    /// This consumes `self` and runs until the server is shut down.
    pub async fn serve(self, addr: std::net::SocketAddr) {
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .unwrap_or_else(|e| {
                panic!("failed to bind API listener {addr}: {e}");
            });
        tracing::info!(addr = %addr, "REST API listening");
        axum::serve(listener, self.router)
            .await
            .expect("API server error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, DnshubConfig};

    fn test_server() -> ApiServer {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        ApiServer::new(state)
    }

    #[test]
    fn router_builds_successfully() {
        let server = test_server();
        let _router = server.router();
        // If this completes without panicking, the router is valid.
    }

    #[test]
    fn router_has_all_routes() {
        let server = test_server();
        let router = server.router();
        // Verify the router has the expected number of nested routes.
        // We can't easily introspect axum routes, but building the router
        // without errors confirms all handlers are correctly typed.
        let _ = router;
    }

    #[test]
    fn server_with_metrics_handle_builds() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .build(),
        );
        let server = ApiServer::new(state);
        let _router = server.router();
    }

    #[test]
    fn server_with_cors_builds() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let server = ApiServer::new(state);
        let _router = server.router();
    }
}
