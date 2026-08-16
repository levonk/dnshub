//! dnshub — a single Rust service that replaces AdGuard Home + dnsdist + CoreDNS.
//!
//! This is the library root. It declares every module up front so that
//! subsequent stories can add implementations without modifying this file.
//!
//! ## Module map
//!
//! | Module      | Story        | Status              |
//! |-------------|--------------|--------------------|
//! | [`dns`]     | 01-001       | server + forwarding + caching (this story) |
//! | [`config`]  | 01-001/01-004| serde structs (this story), full loading (01-004) |
//! | `blocklist` | 01-002       | TODO: implemented in later stories |
//! | `metrics`   | 01-003       | TODO: implemented in later stories |
//! | `policy`    | 02-001       | per-client policy engine + handler (this story) |
//! | `client_resolver` | 02-001 | client IP → profile resolution (this story) |
//! | `dhcp`      | 04-001       | DHCPv4 server core (this story) |
//! | `query_log` | 05-003       | TODO: implemented in later stories |
//! | `api`       | 05-004       | TODO: implemented in later stories |
//! | `frontend`  | 05-006       | TODO: implemented in later stories |

pub mod config;
pub mod dns;

// TODO: implemented in later stories
pub mod blocklist;
// TODO: implemented in later stories
pub mod metrics;
// Per-client policy engine + handler (story 02-001).
pub mod policy;
// Client IP → policy profile resolution (story 02-001).
pub mod client_resolver;
// DHCPv4 server core (story 04-001).
pub mod dhcp;
// TODO: implemented in later stories
pub mod query_log;
// TODO: implemented in later stories
pub mod api;
// TODO: implemented in later stories
pub mod frontend;

/// Crate version (mirrors the Cargo.toml `version` field at build time).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
