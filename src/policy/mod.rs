//! Per-client policy engine and client resolver (PRD section 4.5).
//!
//! This module implements the per-client DNS filtering system:
//!
//! - [`profile::PolicyProfile`] — the per-client policy definition (categories
//!   to block/allow, custom allow/block lists, upstream tier, log level).
//! - [`config::PolicyConfig`] — serde structs for `policy.toml`.
//! - [`engine::PolicyEngine`] — evaluates a domain against a profile, returning
//!   a [`engine::PolicyDecision`] (Allow/Block/Redirect).
//! - [`handler::PolicyHandler`] — a [`DnsMiddleware`](crate::dns::DnsMiddleware)
//!   that intercepts queries, resolves the client profile, and blocks/allowes.
//!
//! Client IP → profile resolution is handled by
//! [`crate::client_resolver::ClientResolver`].

pub mod config;
pub mod engine;
pub mod handler;
pub mod profile;

pub use config::{ClientMapping, DhcpIntegration, PolicyConfig, PolicyConfigError};
pub use engine::{PolicyDecision, PolicyEngine, PolicyReason, PolicyResult};
pub use handler::PolicyHandler;
pub use profile::PolicyProfile;
