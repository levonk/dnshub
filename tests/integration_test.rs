//! Integration test: start the dnshub server on an ephemeral port, send a DNS
//! A query for "example.com" via a hickory-resolver client pointed at it, and
//! assert a response is received.
//!
//! The server forwards to Cloudflare `1.1.1.1:53` (the default tier-1
//! upstream). The test requires outbound UDP/53 to the internet, which is
//! available in the development/CI environment.

use dnshub::config::{CacheConfig, DnshubConfig, UpstreamConfig};
use dnshub::dns::forwarding::ForwardingHandler;
use dnshub::dns::server::DnshubServer;
use dnshub::dns::DnshubHandler;
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::TokioResolver;
use hickory_server::zone_handler::Catalog;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// Build a dnshub config that forwards to Cloudflare 1.1.1.1:53.
fn test_config() -> DnshubConfig {
    DnshubConfig {
        upstreams: vec![UpstreamConfig {
            name: "cloudflare-test".to_string(),
            address: "1.1.1.1:53".to_string(),
            protocol: "udp".to_string(),
            timeout_ms: 5000,
            tier: 1,
        }],
        cache: CacheConfig {
            max_entries: 1024,
            ..CacheConfig::default()
        },
        ..DnshubConfig::default()
    }
}

/// Build a resolver client that queries the dnshub server at `server_addr`.
fn resolver_against(server_addr: SocketAddr) -> TokioResolver {
    // hickory-resolver 0.26 splits the upstream into IpAddr + per-connection
    // ConnectionConfig (which carries the port). We use the udp_and_tcp helper
    // and then override the port on each connection.
    let mut name_server =
        NameServerConfig::udp_and_tcp(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));
    for conn in &mut name_server.connections {
        conn.port = server_addr.port();
    }

    let config = ResolverConfig::from_parts(None, vec![], vec![name_server]);

    TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .build()
        .expect("failed to build resolver")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_query_and_returns_response() {
    let config = test_config();

    // Build the handler: Catalog + ForwardZoneHandler at root.
    let upstream = UpstreamConfig::first_tier_one(&config.upstreams).unwrap();
    let catalog = Catalog::new();
    let catalog = ForwardingHandler::install(catalog, upstream, &config.cache)
        .expect("failed to install forwarding handler");
    let handler = DnshubHandler::from_catalog(catalog);

    // Start the server on an ephemeral UDP+TCP port on the loopback interface.
    let mut server = DnshubServer::new(handler);
    let udp_addr = server
        .register_udp("127.0.0.1:0", &config.server)
        .await
        .expect("failed to bind UDP");
    let _tcp_addr = server
        .register_tcp("127.0.0.1:0", &config.server)
        .await
        .expect("failed to bind TCP");

    let shutdown = CancellationToken::new();
    let server_shutdown = shutdown.clone();
    let server_task = tokio::spawn(async move {
        server.run_until(server_shutdown).await;
    });

    // Give the server a moment to start listening.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send a DNS A query for example.com via hickory-resolver.
    let resolver = resolver_against(udp_addr);
    let lookup = resolver.lookup_ip("example.com.").await;

    // Shut the server down regardless of the query outcome.
    shutdown.cancel();
    let _ = server_task.await;

    let response = match lookup {
        Ok(r) => r,
        Err(e) => {
            // If there is no outbound connectivity, surface the error clearly
            // rather than panicking on an unwrap. This keeps the test honest
            // about its network dependency.
            warn!(error = %e, "resolver lookup failed");
            panic!("expected a response from dnshub, got error: {e}");
        }
    };

    assert!(
        response.iter().count() > 0,
        "expected at least one address in the response"
    );
}
