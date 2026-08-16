//! dnshub binary entry point.
//!
//! Loads configuration (stub: defaults for now — full TOML loading is story
//! 01-004), builds the handler chain (Caching → Forwarding via the
//! `ForwardZoneHandler` installed in the `Catalog`), starts the hickory-server
//! `Server` on UDP+TCP :53, and handles graceful shutdown via Ctrl+C.

use dnshub::config::{DnshubConfig, UpstreamConfig};
use dnshub::dns::forwarding::ForwardingHandler;
use dnshub::dns::server::DnshubServer;
use dnshub::dns::DnshubHandler;
use hickory_server::zone_handler::Catalog;
use std::process;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

#[tokio::main]
async fn main() {
    // Initialize the tracing subscriber with JSON output to stdout.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().json())
        .init();

    info!(version = dnshub::VERSION, "starting dnshub");

    // Load configuration. Full TOML loading + validation is story 01-004;
    // for the scaffold we use the built-in defaults (forwards to 1.1.1.1:53).
    let config = load_config();
    info!(upstreams = config.upstreams.len(), "configuration loaded");

    // Build the handler chain.
    //
    // The Catalog is the hickory-native RequestHandler. We install a
    // ForwardZoneHandler at the root zone (.) so every query is forwarded to
    // the tier-1 upstream, with caching configured on the resolver options.
    // DnshubHandler wraps the Catalog and exposes a Vec<Box<dyn DnsMiddleware>>
    // chain for future stories (rate limit, policy, ECS strip, …).
    let handler = match build_handler(&config) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to build DNS handler");
            process::exit(1);
        }
    };

    // Start the server on the configured listen addresses.
    let mut server = DnshubServer::new(handler);
    for addr in &config.server.listen {
        // Register UDP first (the primary DNS transport).
        if let Err(e) = server.register_udp(addr).await {
            error!(addr = %addr, error = %e, "failed to bind UDP listener");
            process::exit(1);
        }
        // Then TCP (for large responses / zone transfers).
        if let Err(e) = server.register_tcp(addr).await {
            error!(addr = %addr, error = %e, "failed to bind TCP listener");
            process::exit(1);
        }
    }

    // Graceful shutdown: Ctrl+C (SIGINT) cancels the token.
    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
        info!("received Ctrl+C, initiating graceful shutdown");
        shutdown_clone.cancel();
    });

    info!("dnshub is ready, serving DNS");
    server.run_until(shutdown).await;
    info!("dnshub stopped");
}

/// Load the dnshub configuration.
///
/// Story 01-001 uses the built-in defaults. Story 01-004 will read
/// `/etc/dnshub/dnshub.toml` (overridable via `DNSHUB_CONFIG`), validate it,
/// and fall back to defaults on missing sections.
fn load_config() -> DnshubConfig {
    DnshubConfig::defaults()
}

/// Build the [`DnshubHandler`] from the configuration: a [`Catalog`] with a
/// `ForwardZoneHandler` at the root, wrapped with the (currently empty)
/// middleware chain.
fn build_handler(config: &DnshubConfig) -> Result<DnshubHandler, String> {
    let upstream = UpstreamConfig::first_tier_one(&config.upstreams)
        .ok_or_else(|| "no tier-1 upstream configured".to_string())?;

    let catalog = Catalog::new();
    let catalog = ForwardingHandler::install(catalog, upstream, &config.cache)?;

    // The middleware chain is empty for this story; future stories append
    // RateLimitHandler, PolicyHandler, EcsStripHandler, etc.
    Ok(DnshubHandler::from_catalog(catalog))
}
