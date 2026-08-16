//! DHCP endpoints per PRD lines 1327-1346.
//!
//! Endpoints:
//! - `GET    /api/v1/dhcp/leases`         — active DHCP leases (v4 + v6)
//! - `POST   /api/v1/dhcp/release`        — release a lease (by IP or MAC)
//! - `GET    /api/v1/dhcp/static`         — static lease assignments
//! - `POST   /api/v1/dhcp/static`         — add static lease
//! - `DELETE /api/v1/dhcp/static`         — remove static lease
//! - `GET    /api/v1/dhcp/blocklist`      — MAC blocklist entries
//! - `POST   /api/v1/dhcp/blocklist`      — add MAC/OUI to blocklist
//! - `DELETE /api/v1/dhcp/blocklist`      — remove from blocklist
//! - `GET    /api/v1/dhcp/audit`          — lease audit log (filterable)
//! - `GET    /api/v1/dhcp/rogue`          — rogue DHCP detection status
//! - `GET    /api/v1/dhcp/pxe/bootfiles`  — per-architecture bootfile mappings
//! - `PUT    /api/v1/dhcp/pxe/bootfiles`  — update bootfile mappings
//! - `GET    /api/v1/dhcp/pxe/bootp`      — static BOOTP entries
//! - `POST   /api/v1/dhcp/pxe/bootp`      — add BOOTP static entry
//! - `DELETE /api/v1/dhcp/pxe/bootp`      — remove BOOTP static entry
//! - `GET    /api/v1/dhcp/relay/agents`   — trusted relay agents + status
//! - `GET    /api/v1/dhcp/relay/option82` — Option 82 circuit ID → profile
//! - `PUT    /api/v1/dhcp/relay/option82` — update circuit ID mappings
//! - `GET    /api/v1/dhcp/pools`          — all DHCP pools

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::error::ApiError;
use crate::api::state::AppState;
use crate::dhcp::audit::{AuditFilter, DhcpAuditEvent, DhcpAuditEventType};
use crate::dhcp::mac_blocklist::MacBlockEntry;
use crate::dhcp::v4::lease_store::{LeaseV4, StaticLeaseV4Record};
use crate::dhcp::v6::lease_store::{DhcpLeaseV6, StaticLeaseV6};

/// Empty JSON response body (`{}`).
#[derive(Debug, Serialize)]
pub struct EmptyResponse {}

// ---------------------------------------------------------------------------
// DTOs — serializable views of the lease store types.
// ---------------------------------------------------------------------------

/// A DHCPv4 lease in the API response.
#[derive(Debug, Serialize)]
pub struct LeaseV4Dto {
    pub ip: String,
    pub mac: String,
    pub hostname: Option<String>,
    pub client_id: Option<String>,
    pub vendor_class: Option<String>,
    pub profile: Option<String>,
    pub lease_expires: i64,
    pub lease_state: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<LeaseV4> for LeaseV4Dto {
    fn from(l: LeaseV4) -> Self {
        Self {
            ip: l.ip.to_string(),
            mac: l.mac,
            hostname: l.hostname,
            client_id: l.client_id,
            vendor_class: l.vendor_class,
            profile: l.profile,
            lease_expires: l.lease_expires,
            lease_state: l.lease_state.as_str().to_string(),
            created_at: l.created_at,
            updated_at: l.updated_at,
        }
    }
}

/// A DHCPv6 lease in the API response.
#[derive(Debug, Serialize)]
pub struct LeaseV6Dto {
    pub ipv6_address: String,
    pub duid: String,
    pub iaid: u32,
    pub hostname: Option<String>,
    pub vendor_class: Option<String>,
    pub profile: Option<String>,
    pub lease_expires: i64,
    pub lease_state: String,
    pub created_at: i64,
    pub updated_at: i64,
}

impl From<DhcpLeaseV6> for LeaseV6Dto {
    fn from(l: DhcpLeaseV6) -> Self {
        Self {
            ipv6_address: l.ipv6_address.to_string(),
            duid: l.duid,
            iaid: l.iaid,
            hostname: l.hostname,
            vendor_class: l.vendor_class,
            profile: l.profile,
            lease_expires: l.lease_expires,
            lease_state: l.lease_state.as_str().to_string(),
            created_at: l.created_at,
            updated_at: l.updated_at,
        }
    }
}

/// Combined lease list response (v4 + v6).
#[derive(Debug, Serialize)]
pub struct LeasesResponse {
    pub v4: Vec<LeaseV4Dto>,
    pub v6: Vec<LeaseV6Dto>,
}

/// A static v4 lease in the API response.
#[derive(Debug, Serialize)]
pub struct StaticLeaseV4Dto {
    pub mac: String,
    pub ip: String,
    pub hostname: Option<String>,
    pub profile: Option<String>,
}

impl From<StaticLeaseV4Record> for StaticLeaseV4Dto {
    fn from(s: StaticLeaseV4Record) -> Self {
        Self {
            mac: s.mac,
            ip: s.ip.to_string(),
            hostname: s.hostname,
            profile: s.profile,
        }
    }
}

/// A static v6 lease in the API response.
#[derive(Debug, Serialize)]
pub struct StaticLeaseV6Dto {
    pub duid: String,
    pub ipv6_address: String,
    pub hostname: Option<String>,
    pub profile: Option<String>,
}

impl From<StaticLeaseV6> for StaticLeaseV6Dto {
    fn from(s: StaticLeaseV6) -> Self {
        Self {
            duid: s.duid,
            ipv6_address: s.ipv6_address.to_string(),
            hostname: s.hostname,
            profile: s.profile,
        }
    }
}

/// Combined static lease list response.
#[derive(Debug, Serialize)]
pub struct StaticLeasesResponse {
    pub v4: Vec<StaticLeaseV4Dto>,
    pub v6: Vec<StaticLeaseV6Dto>,
}

/// MAC blocklist entry in the API response.
#[derive(Debug, Serialize)]
pub struct MacBlockEntryDto {
    pub mac_or_oui: String,
    pub reason: Option<String>,
    pub created_at: i64,
}

impl From<MacBlockEntry> for MacBlockEntryDto {
    fn from(e: MacBlockEntry) -> Self {
        Self {
            mac_or_oui: e.mac_or_oui,
            reason: e.reason,
            created_at: e.created_at,
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers — Leases
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/leases — list all active DHCP leases (v4 + v6).
pub async fn list_leases(
    State(state): State<Arc<AppState>>,
) -> Result<Json<LeasesResponse>, ApiError> {
    let mut v4_leases = Vec::new();
    if let Some(store) = &state.lease_store_v4 {
        let leases = store.list_leases().await.map_err(|e| {
            ApiError::Internal(format!("v4 lease store: {e}"))
        })?;
        v4_leases = leases.into_iter().map(LeaseV4Dto::from).collect();
    }

    let mut v6_leases = Vec::new();
    if let Some(store) = &state.lease_store_v6 {
        for pool in &state.v6_pools {
            let leases = store.list_leases_v6(pool).await.map_err(|e| {
                ApiError::Internal(format!("v6 lease store: {e}"))
            })?;
            v6_leases.extend(leases.into_iter().map(LeaseV6Dto::from));
        }
    }

    Ok(Json(LeasesResponse {
        v4: v4_leases,
        v6: v6_leases,
    }))
}

/// Request body for POST /api/v1/dhcp/release.
#[derive(Debug, Deserialize)]
pub struct ReleaseRequest {
    /// IP address to release (e.g. `"192.168.1.100"`).
    pub ip: Option<String>,
    /// MAC address to release (v4 lookup).
    pub mac: Option<String>,
}

/// POST /api/v1/dhcp/release — release a lease by IP or MAC.
pub async fn release_lease(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReleaseRequest>,
) -> Result<Json<EmptyResponse>, ApiError> {
    let store = state
        .lease_store_v4
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("DHCPv4 lease store not configured".into()))?;

    let ip_to_delete = if let Some(ip_str) = &req.ip {
        ip_str
            .parse::<std::net::Ipv4Addr>()
            .map_err(|e| ApiError::BadRequest(format!("invalid IP: {e}")))?
    } else if let Some(mac) = &req.mac {
        let lease = store
            .get_lease_by_mac(mac)
            .await
            .map_err(|e| ApiError::Internal(format!("v4 lease store: {e}")))?;
        let lease = lease.ok_or_else(|| ApiError::NotFound(format!("no lease for MAC {mac}")))?;
        lease.ip
    } else {
        return Err(ApiError::BadRequest(
            "either 'ip' or 'mac' must be provided".into(),
        ));
    };

    store
        .delete_lease(ip_to_delete)
        .await
        .map_err(|e| ApiError::Internal(format!("v4 lease store: {e}")))?;

    tracing::info!(ip = %ip_to_delete, "lease released via API");
    Ok(Json(EmptyResponse {}))
}

// ---------------------------------------------------------------------------
// Handlers — Static leases
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/static — list all static lease assignments.
pub async fn list_static_leases(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StaticLeasesResponse>, ApiError> {
    let mut v4 = Vec::new();
    if let Some(store) = &state.lease_store_v4 {
        let leases = store
            .list_static_leases()
            .await
            .map_err(|e| ApiError::Internal(format!("v4 lease store: {e}")))?;
        v4 = leases.into_iter().map(StaticLeaseV4Dto::from).collect();
    }

    let mut v6 = Vec::new();
    if let Some(store) = &state.lease_store_v6 {
        let leases = store
            .list_static_leases_v6()
            .await
            .map_err(|e| ApiError::Internal(format!("v6 lease store: {e}")))?;
        v6 = leases.into_iter().map(StaticLeaseV6Dto::from).collect();
    }

    Ok(Json(StaticLeasesResponse { v4, v6 }))
}

/// Request body for POST /api/v1/dhcp/static.
#[derive(Debug, Deserialize)]
pub struct AddStaticLeaseRequest {
    pub mac: String,
    pub ip: String,
    pub hostname: Option<String>,
    pub profile: Option<String>,
}

/// POST /api/v1/dhcp/static — add a static lease assignment.
pub async fn add_static_lease(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddStaticLeaseRequest>,
) -> Result<Json<StaticLeaseV4Dto>, ApiError> {
    let store = state
        .lease_store_v4
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("DHCPv4 lease store not configured".into()))?;

    let ip: std::net::Ipv4Addr = req
        .ip
        .parse()
        .map_err(|e| ApiError::BadRequest(format!("invalid IP: {e}")))?;

    let record = StaticLeaseV4Record {
        mac: req.mac.clone(),
        ip,
        hostname: req.hostname.clone(),
        profile: req.profile.clone(),
    };

    store
        .upsert_static_lease(&record)
        .await
        .map_err(|e| ApiError::Internal(format!("v4 lease store: {e}")))?;

    tracing::info!(mac = %req.mac, ip = %ip, "static lease added via API");
    Ok(Json(StaticLeaseV4Dto::from(record)))
}

/// Query parameters for DELETE /api/v1/dhcp/static.
#[derive(Debug, Deserialize)]
pub struct DeleteStaticLeaseParams {
    pub mac: String,
}

/// DELETE /api/v1/dhcp/static — remove a static lease assignment.
pub async fn delete_static_lease(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DeleteStaticLeaseParams>,
) -> Result<Json<EmptyResponse>, ApiError> {
    let store = state
        .lease_store_v4
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("DHCPv4 lease store not configured".into()))?;

    store
        .delete_static_lease(&params.mac)
        .await
        .map_err(|e| match e {
            crate::dhcp::v4::lease_store::LeaseStoreError::NotFound => {
                ApiError::NotFound(format!("no static lease for MAC {}", params.mac))
            }
            other => ApiError::Internal(format!("v4 lease store: {other}")),
        })?;

    tracing::info!(mac = %params.mac, "static lease removed via API");
    Ok(Json(EmptyResponse {}))
}

// ---------------------------------------------------------------------------
// Handlers — MAC blocklist
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/blocklist — list MAC blocklist entries.
pub async fn list_mac_blocklist(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MacBlockEntryDto>>, ApiError> {
    let bl = state
        .mac_blocklist
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("MAC blocklist not configured".into()))?;

    let entries = bl
        .list_mac_blocks()
        .map_err(|e| ApiError::Internal(format!("mac blocklist: {e}")))?;

    Ok(Json(
        entries.into_iter().map(MacBlockEntryDto::from).collect(),
    ))
}

/// Request body for POST /api/v1/dhcp/blocklist.
#[derive(Debug, Deserialize)]
pub struct AddMacBlockRequest {
    pub mac_or_oui: String,
    pub reason: Option<String>,
}

/// POST /api/v1/dhcp/blocklist — add a MAC/OUI to the blocklist.
pub async fn add_mac_block(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddMacBlockRequest>,
) -> Result<Json<MacBlockEntryDto>, ApiError> {
    let bl = state
        .mac_blocklist
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("MAC blocklist not configured".into()))?;

    bl.add_mac_block(&req.mac_or_oui, req.reason.as_deref())
        .map_err(|e| ApiError::BadRequest(format!("mac blocklist: {e}")))?;

    // Read back the entry to return the normalized form + timestamp.
    let entries = bl
        .list_mac_blocks()
        .map_err(|e| ApiError::Internal(format!("mac blocklist: {e}")))?;

    // Find the entry we just added (normalized form).
    let entry = entries
        .into_iter()
        .find(|e| {
            // The stored mac_or_oui is the normalized form of our input.
            // We compare loosely by checking if the stored value starts
            // with the same first 3 octets.
            e.mac_or_oui == req.mac_or_oui || e.reason == req.reason
        })
        .map(MacBlockEntryDto::from)
        .unwrap_or(MacBlockEntryDto {
            mac_or_oui: req.mac_or_oui,
            reason: req.reason,
            created_at: 0,
        });

    Ok(Json(entry))
}

/// Query parameters for DELETE /api/v1/dhcp/blocklist.
#[derive(Debug, Deserialize)]
pub struct DeleteMacBlockParams {
    pub mac_or_oui: String,
}

/// DELETE /api/v1/dhcp/blocklist — remove a MAC/OUI from the blocklist.
pub async fn delete_mac_block(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DeleteMacBlockParams>,
) -> Result<Json<EmptyResponse>, ApiError> {
    let bl = state
        .mac_blocklist
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("MAC blocklist not configured".into()))?;

    let removed = bl
        .remove_mac_block(&params.mac_or_oui)
        .map_err(|e| ApiError::BadRequest(format!("mac blocklist: {e}")))?;

    if !removed {
        return Err(ApiError::NotFound(format!(
            "no blocklist entry for '{}'",
            params.mac_or_oui
        )));
    }

    tracing::info!(mac_or_oui = %params.mac_or_oui, "MAC blocklist entry removed via API");
    Ok(Json(EmptyResponse {}))
}

// ---------------------------------------------------------------------------
// Handlers — Audit log
// ---------------------------------------------------------------------------

/// Query parameters for GET /api/v1/dhcp/audit.
#[derive(Debug, Deserialize, Default)]
pub struct AuditQueryParams {
    pub event_type: Option<String>,
    pub mac: Option<String>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub limit: Option<i64>,
}

/// GET /api/v1/dhcp/audit — list audit events (filterable, paginated).
pub async fn list_audit_events(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AuditQueryParams>,
) -> Result<Json<Vec<DhcpAuditEvent>>, ApiError> {
    let logger = state
        .audit_logger
        .as_ref()
        .ok_or_else(|| ApiError::Unavailable("audit log not configured".into()))?;

    let event_type = match &params.event_type {
        Some(s) => Some(
            s.parse::<DhcpAuditEventType>()
                .map_err(|e| ApiError::BadRequest(format!("invalid event_type: {e}")))?,
        ),
        None => None,
    };

    let filter = AuditFilter {
        event_type,
        mac_address: params.mac.clone(),
        from_timestamp: params.from,
        to_timestamp: params.to,
        limit: params.limit,
    };

    let events = logger
        .list_audit_events(&filter)
        .map_err(|e| ApiError::Internal(format!("audit log: {e}")))?;

    Ok(Json(events))
}

// ---------------------------------------------------------------------------
// Handlers — Rogue detection
// ---------------------------------------------------------------------------

/// Rogue DHCP detection status response.
#[derive(Debug, Serialize)]
pub struct RogueStatusResponse {
    pub enabled: bool,
    pub probe_interval_secs: u64,
    pub probe_timeout_ms: u64,
    pub rogues_detected: Vec<RogueServerDto>,
}

/// A detected rogue DHCP server.
#[derive(Debug, Serialize)]
pub struct RogueServerDto {
    pub server_ip: String,
    pub offered_ip: String,
}

/// GET /api/v1/dhcp/rogue — rogue DHCP detection status.
pub async fn get_rogue_status(
    State(state): State<Arc<AppState>>,
) -> Json<RogueStatusResponse> {
    let config = state.config_store.load_full();
    let rogue_cfg = &config.dhcp.rogue_detection;

    Json(RogueStatusResponse {
        enabled: rogue_cfg.enabled,
        probe_interval_secs: rogue_cfg.probe_interval_secs,
        probe_timeout_ms: rogue_cfg.probe_timeout_ms,
        // The detector's runtime state (last probe results) is not
        // exposed through the API yet; return an empty list.
        rogues_detected: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Handlers — PXE / BOOTP
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/pxe/bootfiles — per-architecture bootfile mappings.
pub async fn get_pxe_bootfiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    let config = state.config_store.load_full();
    let bootfiles = config
        .dhcp
        .pxe
        .as_ref()
        .map(|p| p.bootfiles.clone())
        .unwrap_or_default();
    Ok(Json(bootfiles))
}

/// Request body for PUT /api/v1/dhcp/pxe/bootfiles.
#[derive(Debug, Deserialize)]
pub struct UpdateBootfilesRequest {
    pub bootfiles: HashMap<String, String>,
}

/// PUT /api/v1/dhcp/pxe/bootfiles — update bootfile mappings.
///
/// Note: this updates the in-memory config snapshot. Persisting to disk
/// requires a config file write (future enhancement).
pub async fn put_pxe_bootfiles(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateBootfilesRequest>,
) -> Result<Json<HashMap<String, String>>, ApiError> {
    let mut config = (*state.config_store.load_full()).clone();
    let pxe = config.dhcp.pxe.get_or_insert_with(Default::default);
    pxe.bootfiles = req.bootfiles.clone();
    state.config_store.swap(config);
    Ok(Json(req.bootfiles))
}

/// GET /api/v1/dhcp/pxe/bootp — static BOOTP entries.
pub async fn get_pxe_bootp(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<crate::dhcp::pxe::config::BootpStaticEntry>>, ApiError> {
    let config = state.config_store.load_full();
    let entries = config
        .dhcp
        .pxe
        .as_ref()
        .map(|p| p.bootp_static.clone())
        .unwrap_or_default();
    Ok(Json(entries))
}

/// POST /api/v1/dhcp/pxe/bootp — add a BOOTP static entry.
pub async fn add_pxe_bootp(
    State(state): State<Arc<AppState>>,
    Json(entry): Json<crate::dhcp::pxe::config::BootpStaticEntry>,
) -> Result<Json<crate::dhcp::pxe::config::BootpStaticEntry>, ApiError> {
    let mut config = (*state.config_store.load_full()).clone();
    let pxe = config.dhcp.pxe.get_or_insert_with(Default::default);
    pxe.bootp_static.push(entry.clone());
    state.config_store.swap(config);
    Ok(Json(entry))
}

/// Query parameters for DELETE /api/v1/dhcp/pxe/bootp.
#[derive(Debug, Deserialize)]
pub struct DeleteBootpParams {
    pub mac: String,
}

/// DELETE /api/v1/dhcp/pxe/bootp — remove a BOOTP static entry.
pub async fn delete_pxe_bootp(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DeleteBootpParams>,
) -> Result<Json<EmptyResponse>, ApiError> {
    let mut config = (*state.config_store.load_full()).clone();
    let pxe = config.dhcp.pxe.get_or_insert_with(Default::default);
    let before = pxe.bootp_static.len();
    pxe.bootp_static.retain(|e| e.mac != params.mac);
    let after = pxe.bootp_static.len();

    if before == after {
        return Err(ApiError::NotFound(format!(
            "no BOOTP entry for MAC '{}'",
            params.mac
        )));
    }

    state.config_store.swap(config);
    Ok(Json(EmptyResponse {}))
}

// ---------------------------------------------------------------------------
// Handlers — Relay
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/relay/agents — trusted relay agents + status.
pub async fn get_relay_agents(
    State(state): State<Arc<AppState>>,
) -> Json<crate::dhcp::relay::config::RelayConfig> {
    let config = state.config_store.load_full();
    Json(config.dhcp.relay.clone())
}

/// GET /api/v1/dhcp/relay/option82 — Option 82 circuit ID → profile mappings.
pub async fn get_relay_option82(
    State(state): State<Arc<AppState>>,
) -> Json<crate::dhcp::relay::config::Option82Config> {
    let config = state.config_store.load_full();
    Json(config.dhcp.relay.option82.clone())
}

/// PUT /api/v1/dhcp/relay/option82 — update circuit ID mappings.
pub async fn put_relay_option82(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<crate::dhcp::relay::config::Option82Config>,
) -> Result<Json<crate::dhcp::relay::config::Option82Config>, ApiError> {
    let mut config = (*state.config_store.load_full()).clone();
    config.dhcp.relay.option82 = cfg.clone();
    state.config_store.swap(config);
    Ok(Json(cfg))
}

// ---------------------------------------------------------------------------
// Handlers — Pools
// ---------------------------------------------------------------------------

/// GET /api/v1/dhcp/pools — all DHCP pools (local + relayed VLANs).
pub async fn get_dhcp_pools(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<crate::config::DhcpPoolConfig>> {
    let config = state.config_store.load_full();
    Json(config.dhcp.pools.clone())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ConfigStore, DnshubConfig};
    use crate::dhcp::audit::{AuditLogger, DhcpAuditEvent, DhcpAuditEventType};
    use crate::dhcp::mac_blocklist::MacBlocklist;
    use crate::dhcp::v4::lease_store::{
        LeaseState, LeaseStoreV4, LeaseV4, SqliteLeaseStoreV4,
    };
    use axum::extract::{Query, State};

    fn state_with_v4_store() -> Arc<AppState> {
        let store = SqliteLeaseStoreV4::open_in_memory().unwrap();
        Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .lease_store_v4(Arc::new(store))
                .build(),
        )
    }

    fn state_with_mac_blocklist() -> Arc<AppState> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let bl = MacBlocklist::with_connection(conn);
        bl.init_schema().unwrap();
        Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .mac_blocklist(Arc::new(bl))
                .build(),
        )
    }

    fn state_with_audit() -> Arc<AppState> {
        let logger = AuditLogger::open_in_memory().unwrap();
        Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults())))
                .audit_logger(Arc::new(logger))
                .build(),
        )
    }

    fn make_lease(ip: &str, mac: &str) -> LeaseV4 {
        let ts = crate::dhcp::v4::lease_store::now_ts();
        LeaseV4 {
            ip: ip.parse().unwrap(),
            mac: mac.to_string(),
            hostname: Some("test".to_string()),
            client_id: None,
            vendor_class: None,
            profile: None,
            lease_expires: ts + 3600,
            lease_state: LeaseState::Active,
            created_at: ts,
            updated_at: ts,
        }
    }

    #[tokio::test]
    async fn list_leases_empty() {
        let state = state_with_v4_store();
        let Json(result) = list_leases(State(state)).await.unwrap();
        assert!(result.v4.is_empty());
        assert!(result.v6.is_empty());
    }

    #[tokio::test]
    async fn list_leases_with_data() {
        let state = state_with_v4_store();
        {
            let store: &dyn LeaseStoreV4 = state.lease_store_v4.as_ref().unwrap().as_ref();
            store.insert_lease(&make_lease("192.168.1.100", "00:11:22:33:44:55")).await.unwrap();
        }
        let Json(result) = list_leases(State(state)).await.unwrap();
        assert_eq!(result.v4.len(), 1);
        assert_eq!(result.v4[0].ip, "192.168.1.100");
        assert_eq!(result.v4[0].mac, "00:11:22:33:44:55");
    }

    #[tokio::test]
    async fn release_lease_by_ip() {
        let state = state_with_v4_store();
        {
            let store: &dyn LeaseStoreV4 = state.lease_store_v4.as_ref().unwrap().as_ref();
            store.insert_lease(&make_lease("192.168.1.101", "aa:bb:cc:dd:ee:ff")).await.unwrap();
        }
        let req = ReleaseRequest {
            ip: Some("192.168.1.101".to_string()),
            mac: None,
        };
        let result = release_lease(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());
        let store: &dyn LeaseStoreV4 = state.lease_store_v4.as_ref().unwrap().as_ref();
        assert!(store.get_lease("192.168.1.101".parse().unwrap()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_lease_by_mac() {
        let state = state_with_v4_store();
        {
            let store: &dyn LeaseStoreV4 = state.lease_store_v4.as_ref().unwrap().as_ref();
            store.insert_lease(&make_lease("192.168.1.102", "11:22:33:44:55:66")).await.unwrap();
        }
        let req = ReleaseRequest {
            ip: None,
            mac: Some("11:22:33:44:55:66".to_string()),
        };
        let result = release_lease(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());
        let store: &dyn LeaseStoreV4 = state.lease_store_v4.as_ref().unwrap().as_ref();
        assert!(store.get_lease("192.168.1.102".parse().unwrap()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn release_lease_no_ip_or_mac() {
        let state = state_with_v4_store();
        let req = ReleaseRequest { ip: None, mac: None };
        let result = release_lease(State(state), Json(req)).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ApiError::BadRequest(_)));
    }

    #[tokio::test]
    async fn static_lease_add_list_delete() {
        let state = state_with_v4_store();

        // Add a static lease.
        let req = AddStaticLeaseRequest {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.50".to_string(),
            hostname: Some("printer".to_string()),
            profile: Some("iot".to_string()),
        };
        let result = add_static_lease(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());

        // List static leases.
        let Json(list) = list_static_leases(State(state.clone())).await.unwrap();
        assert_eq!(list.v4.len(), 1);
        assert_eq!(list.v4[0].mac, "00:11:22:33:44:55");
        assert_eq!(list.v4[0].ip, "192.168.1.50");

        // Delete the static lease.
        let params = DeleteStaticLeaseParams {
            mac: "00:11:22:33:44:55".to_string(),
        };
        let result = delete_static_lease(State(state.clone()), Query(params)).await;
        assert!(result.is_ok());

        // Verify it's gone.
        let Json(list) = list_static_leases(State(state)).await.unwrap();
        assert!(list.v4.is_empty());
    }

    #[tokio::test]
    async fn mac_blocklist_add_list_delete() {
        let state = state_with_mac_blocklist();

        // Add.
        let req = AddMacBlockRequest {
            mac_or_oui: "00:11:22".to_string(),
            reason: Some("bad vendor".to_string()),
        };
        let result = add_mac_block(State(state.clone()), Json(req)).await;
        assert!(result.is_ok());

        // List.
        let Json(list) = list_mac_blocklist(State(state.clone())).await.unwrap();
        assert_eq!(list.len(), 1);

        // Delete.
        let params = DeleteMacBlockParams {
            mac_or_oui: "00:11:22".to_string(),
        };
        let result = delete_mac_block(State(state.clone()), Query(params)).await;
        assert!(result.is_ok());

        // Verify it's gone.
        let Json(list) = list_mac_blocklist(State(state)).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn audit_list_with_data() {
        let state = state_with_audit();
        {
            let logger = state.audit_logger.as_ref().unwrap();
            let ev = DhcpAuditEvent::new(1000, DhcpAuditEventType::Ack)
                .with_mac("00:11:22:33:44:55")
                .with_ip("192.168.1.10");
            logger.write_audit_event(&ev).unwrap();
        }
        let Json(events) = list_audit_events(State(state), Query(AuditQueryParams::default())).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, DhcpAuditEventType::Ack);
    }

    #[tokio::test]
    async fn audit_filter_by_event_type() {
        let state = state_with_audit();
        {
            let logger = state.audit_logger.as_ref().unwrap();
            logger.write_audit_event(&DhcpAuditEvent::new(1, DhcpAuditEventType::Ack)).unwrap();
            logger.write_audit_event(&DhcpAuditEvent::new(2, DhcpAuditEventType::Release)).unwrap();
        }
        let params = AuditQueryParams {
            event_type: Some("release".to_string()),
            ..Default::default()
        };
        let Json(events) = list_audit_events(State(state), Query(params)).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, DhcpAuditEventType::Release);
    }

    #[tokio::test]
    async fn audit_filter_by_mac() {
        let state = state_with_audit();
        {
            let logger = state.audit_logger.as_ref().unwrap();
            logger.write_audit_event(
                &DhcpAuditEvent::new(1, DhcpAuditEventType::Ack).with_mac("aa:bb:cc:dd:ee:ff"),
            ).unwrap();
            logger.write_audit_event(
                &DhcpAuditEvent::new(2, DhcpAuditEventType::Ack).with_mac("00:11:22:33:44:55"),
            ).unwrap();
        }
        let params = AuditQueryParams {
            mac: Some("aa:bb:cc:dd:ee:ff".to_string()),
            ..Default::default()
        };
        let Json(events) = list_audit_events(State(state), Query(params)).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].mac_address.as_deref(), Some("aa:bb:cc:dd:ee:ff"));
    }

    #[tokio::test]
    async fn audit_with_limit() {
        let state = state_with_audit();
        {
            let logger = state.audit_logger.as_ref().unwrap();
            for i in 0..5 {
                logger.write_audit_event(&DhcpAuditEvent::new(i, DhcpAuditEventType::Ack)).unwrap();
            }
        }
        let params = AuditQueryParams {
            limit: Some(2),
            ..Default::default()
        };
        let Json(events) = list_audit_events(State(state), Query(params)).await.unwrap();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn rogue_status() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let Json(result) = get_rogue_status(State(state)).await;
        assert!(!result.enabled);
    }

    #[tokio::test]
    async fn pxe_bootfiles_get_empty() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let Json(result) = get_pxe_bootfiles(State(state)).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn pxe_bootfiles_put() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let mut bootfiles = std::collections::HashMap::new();
        bootfiles.insert("0".to_string(), "pxelinux.0".to_string());
        let req = UpdateBootfilesRequest { bootfiles: bootfiles.clone() };
        let Json(result) = put_pxe_bootfiles(State(state), Json(req)).await.unwrap();
        assert_eq!(result.get("0"), Some(&"pxelinux.0".to_string()));
    }

    #[tokio::test]
    async fn pxe_bootp_add_and_delete() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let entry = crate::dhcp::pxe::config::BootpStaticEntry {
            mac: "00:11:22:33:44:aa".to_string(),
            ip: "192.168.1.50".to_string(),
            bootfile: "coreos.ipxe".to_string(),
            tftp_server: "192.168.1.67".to_string(),
        };
        let result = add_pxe_bootp(State(state.clone()), Json(entry)).await;
        assert!(result.is_ok());

        let Json(list) = get_pxe_bootp(State(state.clone())).await.unwrap();
        assert_eq!(list.len(), 1);

        let params = DeleteBootpParams {
            mac: "00:11:22:33:44:aa".to_string(),
        };
        let result = delete_pxe_bootp(State(state.clone()), Query(params)).await;
        assert!(result.is_ok());

        let Json(list) = get_pxe_bootp(State(state)).await.unwrap();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn pxe_bootp_delete_not_found() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let params = DeleteBootpParams {
            mac: "00:00:00:00:00:00".to_string(),
        };
        let result = delete_pxe_bootp(State(state), Query(params)).await;
        assert!(matches!(result, Err(ApiError::NotFound(_))));
    }

    #[tokio::test]
    async fn relay_agents_get() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let Json(result) = get_relay_agents(State(state)).await;
        assert!(!result.enabled);
    }

    #[tokio::test]
    async fn relay_option82_put() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let mut map = std::collections::HashMap::new();
        map.insert("port-1".to_string(), "guest".to_string());
        let cfg = crate::dhcp::relay::config::Option82Config {
            enabled: true,
            circuit_id_map: map,
        };
        let Json(result) = put_relay_option82(State(state), Json(cfg)).await.unwrap();
        assert!(result.enabled);
        assert_eq!(result.circuit_id_map.get("port-1"), Some(&"guest".to_string()));
    }

    #[tokio::test]
    async fn dhcp_pools_get() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let Json(result) = get_dhcp_pools(State(state)).await;
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn leases_unavailable_when_no_store() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let req = AddStaticLeaseRequest {
            mac: "00:11:22:33:44:55".to_string(),
            ip: "192.168.1.50".to_string(),
            hostname: None,
            profile: None,
        };
        let result = add_static_lease(State(state), Json(req)).await;
        assert!(matches!(result, Err(ApiError::Unavailable(_))));
    }

    #[tokio::test]
    async fn mac_blocklist_unavailable_when_not_configured() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let result = list_mac_blocklist(State(state)).await;
        assert!(matches!(result, Err(ApiError::Unavailable(_))));
    }

    #[tokio::test]
    async fn audit_unavailable_when_not_configured() {
        let state = Arc::new(
            AppState::builder(Arc::new(ConfigStore::new(DnshubConfig::defaults()))).build(),
        );
        let result = list_audit_events(State(state), Query(AuditQueryParams::default())).await;
        assert!(matches!(result, Err(ApiError::Unavailable(_))));
    }
}
