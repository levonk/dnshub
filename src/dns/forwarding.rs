//! Upstream forwarding via hickory-server's `ForwardZoneHandler`.
//!
//! In hickory-server 0.26 the old `ForwardAuthority` was renamed to
//! [`ForwardZoneHandler`](hickory_server::store::forwarder::ForwardZoneHandler).
//! It is a [`ZoneHandler`](hickory_server::zone_handler::ZoneHandler) that
//! forwards resolutions to upstream resolvers using the `hickory-resolver`
//! crate. It is inserted into the [`Catalog`](hickory_server::zone_handler::Catalog)
//! at the root zone (`.`) so it handles every query that earlier middleware
//! did not short-circuit.
//!
//! ## Tiered forwarding (story 03-001)
//!
//! [`ForwardingHandler::install`] wires a *single* tier-1 upstream (the
//! original story 01-001 behavior, still used by the server bootstrap path).
//! [`ForwardingHandler::install_tiered`] builds a
//! [`TieredForwardHandler`](crate::dns::tiered_forward::TieredForwardHandler)
//! from the full `[[upstreams]]` config — sorted by tier, with per-tier
//! timeout and fallback — and inserts it at the root zone. This is the
//! production forwarding path for multi-tier deployments.

use crate::config::{CacheConfig, UpstreamConfig};
use crate::dns::tiered_forward::{ForwardUpstream, TieredForwardHandler, TieredUpstream};
use hickory_proto::rr::{LowerName, Name};
use hickory_resolver::config::{ConnectionConfig, NameServerConfig, ResolverOpts};
use hickory_server::store::forwarder::{ForwardConfig, ForwardZoneHandler};
use hickory_server::zone_handler::{Catalog, ZoneHandler};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Builds a [`ForwardZoneHandler`] for a single tier-1 upstream and inserts it
/// into a [`Catalog`] at the root zone.
///
/// This is the 0.26 equivalent of the story's "ForwardingHandler wrapping
/// ForwardAuthority". Caching is configured on the resolver options (see
/// [`crate::dns::caching`]) rather than as a separate client wrapper, because
/// `ForwardZoneHandler` owns its resolver and `CachingClient` lives in
/// `hickory-resolver`'s client layer.
pub struct ForwardingHandler;

impl ForwardingHandler {
    /// Build a `ForwardZoneHandler` for `upstream`, apply `cache` settings to
    /// its resolver options, and insert it into `catalog` at the root zone.
    ///
    /// Returns the catalog so callers can chain further zone handler
    /// insertions.
    pub fn install(
        catalog: Catalog,
        upstream: &UpstreamConfig,
        cache: &CacheConfig,
    ) -> Result<Catalog, String> {
        let socket_addr = upstream
            .socket_addr()
            .map_err(|e| format!("invalid upstream address {}: {e}", upstream.address))?;

        let name_server = build_name_server(socket_addr, &upstream.protocol);

        let mut options = ResolverOpts::default();
        crate::dns::caching::CachingHandler::apply(&mut options, cache);

        let forward_config = ForwardConfig {
            name_servers: vec![name_server],
            options: Some(options),
        };

        let forward_zone = ForwardZoneHandler::builder_tokio(forward_config)
            .with_origin(Name::root())
            .build()?;

        // hickory-server 0.26 `Catalog::upsert` takes the zone origin (as a
        // `LowerName`) plus a Vec of `Arc<dyn ZoneHandler>` handlers. Inserting
        // at the root zone (`.`) means this forwarder handles every query that
        // earlier middleware did not short-circuit.
        let origin = LowerName::from(&Name::root());
        let handler: Arc<dyn ZoneHandler> = Arc::new(forward_zone);
        let mut catalog = catalog;
        catalog.upsert(origin, vec![handler]);

        info!(
            upstream = %upstream.name,
            address = %upstream.address,
            tier = upstream.tier,
            "installed forwarding zone handler at root"
        );
        Ok(catalog)
    }

    /// Build a [`TieredForwardHandler`] from the full set of `[[upstreams]]`,
    /// sorted by tier with per-tier timeout and fallback, and insert it into
    /// `catalog` at the root zone (`.`).
    ///
    /// Each upstream becomes one tier. Caching settings (`cache`) are applied
    /// to every tier's resolver options. Tiers are sorted ascending by
    /// `UpstreamConfig::tier` inside [`TieredForwardHandler::new`].
    ///
    /// Returns the catalog so callers can chain further zone handler
    /// insertions. If `upstreams` is empty, returns an error.
    pub fn install_tiered(
        catalog: Catalog,
        upstreams: &[UpstreamConfig],
        cache: &CacheConfig,
    ) -> Result<Catalog, String> {
        if upstreams.is_empty() {
            return Err("no upstreams configured for tiered forwarding".to_string());
        }

        let mut tiers: Vec<TieredUpstream> = Vec::with_capacity(upstreams.len());
        for upstream in upstreams {
            let forward_zone = build_forward_zone(upstream, cache)?;
            let upstream_arc: Arc<dyn ForwardUpstream> = Arc::new(forward_zone);
            tiers.push(TieredUpstream::new(
                upstream.tier,
                &upstream.name,
                Duration::from_millis(upstream.timeout_ms),
                upstream_arc,
            ));
            info!(
                upstream = %upstream.name,
                address = %upstream.address,
                tier = upstream.tier,
                timeout_ms = upstream.timeout_ms,
                "registered tiered upstream"
            );
        }

        let handler = TieredForwardHandler::new(tiers);
        let origin = LowerName::from(&Name::root());
        let zone_handler: Arc<dyn ZoneHandler> = Arc::new(handler);
        let mut catalog = catalog;
        catalog.upsert(origin, vec![zone_handler]);

        info!("installed tiered forward handler at root");
        Ok(catalog)
    }
}

/// Build a [`ForwardZoneHandler`] for a single `upstream`, applying `cache`
/// settings to its resolver options.
fn build_forward_zone(
    upstream: &UpstreamConfig,
    cache: &CacheConfig,
) -> Result<ForwardZoneHandler, String> {
    let socket_addr = upstream
        .socket_addr()
        .map_err(|e| format!("invalid upstream address {}: {e}", upstream.address))?;

    let name_server = build_name_server(socket_addr, &upstream.protocol);

    let mut options = ResolverOpts::default();
    crate::dns::caching::CachingHandler::apply(&mut options, cache);

    let forward_config = ForwardConfig {
        name_servers: vec![name_server],
        options: Some(options),
    };

    ForwardZoneHandler::builder_tokio(forward_config)
        .with_origin(Name::root())
        .build()
}

/// Build a [`NameServerConfig`] for the given socket address and protocol.
///
/// `hickory-resolver` 0.26 splits the upstream address into an [`IpAddr`] plus
/// per-connection [`ConnectionConfig`]s that carry the port. We construct a
/// single connection (UDP and/or TCP) and set the port explicitly so
/// non-standard upstream ports (e.g. `172.20.255.50:15353`) work.
fn build_name_server(addr: std::net::SocketAddr, protocol: &str) -> NameServerConfig {
    let ip: IpAddr = addr.ip();
    let port = addr.port();

    let mut connections: Vec<ConnectionConfig> = Vec::new();
    match protocol.to_ascii_lowercase().as_str() {
        "udp" => {
            let mut c = ConnectionConfig::udp();
            c.port = port;
            connections.push(c);
        }
        "tcp" => {
            let mut c = ConnectionConfig::tcp();
            c.port = port;
            connections.push(c);
        }
        "both" | "udp+tcp" | "udp,tcp" => {
            let mut c_udp = ConnectionConfig::udp();
            c_udp.port = port;
            connections.push(c_udp);
            let mut c_tcp = ConnectionConfig::tcp();
            c_tcp.port = port;
            connections.push(c_tcp);
        }
        other => {
            warn!(protocol = %other, "unknown upstream protocol, defaulting to UDP");
            let mut c = ConnectionConfig::udp();
            c.port = port;
            connections.push(c);
        }
    }

    NameServerConfig::new(ip, true, connections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_udp_name_server_with_custom_port() {
        let ns = build_name_server("172.20.255.50:15353".parse().unwrap(), "udp");
        assert_eq!(ns.ip, IpAddr::V4([172, 20, 255, 50].into()));
        assert_eq!(ns.connections.len(), 1);
        assert_eq!(ns.connections[0].port, 15353);
    }

    #[test]
    fn builds_tcp_name_server_with_custom_port() {
        let ns = build_name_server("1.1.1.1:53".parse().unwrap(), "tcp");
        assert_eq!(ns.connections.len(), 1);
        assert_eq!(ns.connections[0].port, 53);
    }

    #[test]
    fn builds_both_name_server() {
        let ns = build_name_server("1.1.1.1:53".parse().unwrap(), "both");
        assert_eq!(ns.connections.len(), 2);
    }
}
