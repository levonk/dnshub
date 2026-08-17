//! REST API (axum) for config, status, DHCP leases, and query log.
//!
//! This module implements all endpoints defined in PRD section 4.9
//! (lines 1321-1356) using axum 0.7. The API serves the NextJS frontend
//! and provides: config view/update, service status, DHCP lease
//! management, DHCP MAC blocklist, DHCP audit log, DHCP PXE/BOOTP config,
//! DHCP relay config, blocklist source health, query log (paginated),
//! and CSV export.
//!
//! ## Architecture
//!
//! ```text
//!  HTTP request
//!    │
//!    ▼
//!  CorsLayer  ──▶  Router (/api/v1/*)  ──▶  Handler(State<AppState>)
//!    │                                           │
//!    │                                           ▼
//!    │                                     AppState {
//!    │                                       ConfigStore (ArcSwap),
//!    │                                       LeaseStoreV4/V6,
//!    │                                       MacBlocklist,
//!    │                                       AuditLogger,
//!    │                                       BlocklistConfig,
//!    │                                       ...
//!    │                                     }
//!    ▼
//!  JSON response
//! ```
//!
//! ## Usage
//!
//! ```no_run
//! use dnshub::api::{ApiServer, AppState};
//! use dnshub::config::{ConfigStore, DnshubConfig};
//! use std::sync::Arc;
//!
//! # async fn run() {
//! let config_store = Arc::new(ConfigStore::new(DnshubConfig::defaults()));
//! let state = Arc::new(AppState::builder(config_store).build());
//! let server = ApiServer::new(state);
//! server.serve("0.0.0.0:8080".parse().unwrap()).await;
//! # }
//! ```

pub mod auth;
pub mod error;
pub mod routes;
pub mod server;
pub mod state;

pub use error::ApiError;
pub use server::{ApiServer, ApiServerOptions};
pub use state::{AppState, AppStateBuilder};
