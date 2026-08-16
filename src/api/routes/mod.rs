//! Route handler modules for each PRD section 4.9 endpoint group.
//!
//! Each submodule owns the handlers for one logical group:
//!
//! - [`config`] — GET/PUT `/api/v1/config`
//! - [`status`] — GET `/api/v1/status`
//! - [`dhcp`] — all DHCP endpoints (leases, static, blocklist, audit, rogue,
//!   pxe, relay, pools)
//! - [`blocklists`] — GET `/api/v1/blocklists/sources`, POST
//!   `/api/v1/blocklists/refresh`
//! - [`query_log`] — GET `/api/v1/query-log`, GET `/api/v1/query-log/export`

pub mod blocklists;
pub mod config;
pub mod dhcp;
pub mod query_log;
pub mod status;
