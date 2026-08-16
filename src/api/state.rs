//! Shared application state passed to every axum route handler.
//!
//! [`AppState`] is wrapped in `Arc` and inserted into the axum router via
//! `Router::with_state`. Each handler extracts it with
//! `axum::extract::State<AppState>`.
//!
//! All subsystem handles are `Option` so the API can be constructed with
//! only the components available (e.g. in tests, or when DHCP is disabled).
//! Handlers that require a missing subsystem return
//! [`ApiError::Unavailable`][crate::api::error::ApiError::Unavailable].

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::Notify;

use crate::blocklist::config::BlocklistsConfig;
use crate::blocklist::health::SourceHealthRegistry;
use crate::config::ConfigStore;
use crate::dhcp::audit::AuditLogger;
use crate::dhcp::mac_blocklist::MacBlocklist;
use crate::dhcp::v4::lease_store::LeaseStoreV4;
use crate::dhcp::v6::config::DhcpPoolV6;
use crate::dhcp::v6::lease_store::LeaseStoreV6;
use crate::metrics::recorder::PrometheusHandle;

/// Shared state for all API route handlers.
///
/// Construct via [`AppStateBuilder`] to avoid spelling out every `Option`.
#[derive(Clone)]
pub struct AppState {
    /// Atomic config holder (ArcSwap). Always present.
    pub config_store: Arc<ConfigStore>,
    /// Path to `dnshub.toml` on disk (for PUT /config write-back).
    pub config_path: Option<PathBuf>,
    /// DHCPv4 lease store, if the DHCPv4 server is running.
    pub lease_store_v4: Option<Arc<dyn LeaseStoreV4>>,
    /// DHCPv6 lease store, if the DHCPv6 server is running.
    pub lease_store_v6: Option<Arc<dyn LeaseStoreV6>>,
    /// DHCPv6 pools (needed to list v6 leases, which are scoped by pool).
    pub v6_pools: Vec<DhcpPoolV6>,
    /// MAC blocklist store.
    pub mac_blocklist: Option<Arc<MacBlocklist>>,
    /// DHCP audit log.
    pub audit_logger: Option<Arc<AuditLogger>>,
    /// Blocklist sources config (for GET /blocklists/sources).
    pub blocklist_config: Option<Arc<BlocklistsConfig>>,
    /// Notify handle to trigger an immediate blocklist refresh.
    pub blocklist_refresh: Option<Arc<Notify>>,
    /// Per-source health registry (for source health reporting).
    pub source_health: Option<Arc<SourceHealthRegistry>>,
    /// Prometheus handle for rendering metrics in /status.
    pub metrics_handle: Option<Arc<PrometheusHandle>>,
    /// Server start time (for uptime calculation in /status).
    pub start_time: Instant,
}

impl AppState {
    /// Create a builder for constructing an [`AppState`].
    pub fn builder(config_store: Arc<ConfigStore>) -> AppStateBuilder {
        AppStateBuilder {
            config_store,
            config_path: None,
            lease_store_v4: None,
            lease_store_v6: None,
            v6_pools: Vec::new(),
            mac_blocklist: None,
            audit_logger: None,
            blocklist_config: None,
            blocklist_refresh: None,
            source_health: None,
            metrics_handle: None,
        }
    }
}

/// Builder for [`AppState`].
pub struct AppStateBuilder {
    config_store: Arc<ConfigStore>,
    config_path: Option<PathBuf>,
    lease_store_v4: Option<Arc<dyn LeaseStoreV4>>,
    lease_store_v6: Option<Arc<dyn LeaseStoreV6>>,
    v6_pools: Vec<DhcpPoolV6>,
    mac_blocklist: Option<Arc<MacBlocklist>>,
    audit_logger: Option<Arc<AuditLogger>>,
    blocklist_config: Option<Arc<BlocklistsConfig>>,
    blocklist_refresh: Option<Arc<Notify>>,
    source_health: Option<Arc<SourceHealthRegistry>>,
    metrics_handle: Option<Arc<PrometheusHandle>>,
}

impl AppStateBuilder {
    pub fn config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    pub fn lease_store_v4(mut self, store: Arc<dyn LeaseStoreV4>) -> Self {
        self.lease_store_v4 = Some(store);
        self
    }

    pub fn lease_store_v6(mut self, store: Arc<dyn LeaseStoreV6>) -> Self {
        self.lease_store_v6 = Some(store);
        self
    }

    pub fn v6_pools(mut self, pools: Vec<DhcpPoolV6>) -> Self {
        self.v6_pools = pools;
        self
    }

    pub fn mac_blocklist(mut self, bl: Arc<MacBlocklist>) -> Self {
        self.mac_blocklist = Some(bl);
        self
    }

    pub fn audit_logger(mut self, logger: Arc<AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    pub fn blocklist_config(mut self, cfg: Arc<BlocklistsConfig>) -> Self {
        self.blocklist_config = Some(cfg);
        self
    }

    pub fn blocklist_refresh(mut self, notify: Arc<Notify>) -> Self {
        self.blocklist_refresh = Some(notify);
        self
    }

    pub fn source_health(mut self, health: Arc<SourceHealthRegistry>) -> Self {
        self.source_health = Some(health);
        self
    }

    pub fn metrics_handle(mut self, handle: Arc<PrometheusHandle>) -> Self {
        self.metrics_handle = Some(handle);
        self
    }

    /// Build the [`AppState`].
    pub fn build(self) -> AppState {
        AppState {
            config_store: self.config_store,
            config_path: self.config_path,
            lease_store_v4: self.lease_store_v4,
            lease_store_v6: self.lease_store_v6,
            v6_pools: self.v6_pools,
            mac_blocklist: self.mac_blocklist,
            audit_logger: self.audit_logger,
            blocklist_config: self.blocklist_config,
            blocklist_refresh: self.blocklist_refresh,
            source_health: self.source_health,
            metrics_handle: self.metrics_handle,
            start_time: Instant::now(),
        }
    }
}
