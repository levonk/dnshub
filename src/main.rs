//! dnshub binary entry point.
//!
//! Loads configuration (stub: defaults for now — full TOML loading is story
//! 01-004), builds the handler chain (Caching → Forwarding via the
//! `ForwardZoneHandler` installed in the `Catalog`), starts the hickory-server
//! `Server` on UDP+TCP :53, and handles graceful shutdown via Ctrl+C.
//!
//! Story 02-004 installs a SIGHUP hot-reload handler: on SIGHUP the active
//! `DnshubConfig` (held in a `ConfigStore` / `ArcSwap`) is reloaded from disk
//! and atomically swapped, and the blocklist daemon is signalled to refresh.

use dnshub::config::{
    ConfigStore, DnshubConfig, HotReloadManager, HotReloadPaths, UpstreamConfig,
};
use dnshub::dns::dot::{DotServer, log_tls_load_error};
use dnshub::dns::forwarding::ForwardingHandler;
use dnshub::dns::server::DnshubServer;
use dnshub::dns::DnshubHandler;
use hickory_server::zone_handler::Catalog;
use std::path::PathBuf;
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

    // Wrap the config in a ConfigStore (ArcSwap) so the SIGHUP hot-reload
    // handler can atomically swap in a reloaded config without blocking
    // concurrent query processing.
    let config_store = std::sync::Arc::new(ConfigStore::new(config));

    // Build the handler chain.
    //
    // The Catalog is the hickory-native RequestHandler. We install a
    // ForwardZoneHandler at the root zone (.) so every query is forwarded to
    // the tier-1 upstream, with caching configured on the resolver options.
    // DnshubHandler wraps the Catalog and exposes a Vec<Box<dyn DnsMiddleware>>
    // chain for future stories (rate limit, policy, ECS strip, …).
    let handler = match build_handler(&config_store.load_full()) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to build DNS handler");
            process::exit(1);
        }
    };

    // Start the server on the configured listen addresses.
    let mut server = DnshubServer::new(handler);
    for addr in &config_store.load_full().server.listen {
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

    // Start the DoT (DNS-over-TLS) server on :853 when [server.tls].enabled.
    // The TLS cert/key are loaded from the configured paths (reusing existing
    // Traefik/ACME certs mounted into the container — PRD §4.7). DoT clients
    // are served by the same DnshubHandler as UDP/TCP, so per-client policy,
    // blocklists, and forwarding apply identically.
    if let Some(tls) = &config_store.load_full().server.tls {
        if tls.enabled {
            match DotServer::from_config(tls) {
                Ok(dot) => match server.register_tls(&dot).await {
                    Ok(bound) => {
                        for addr in &bound {
                            info!(addr = %addr, "DoT listener ready");
                        }
                    }
                    Err(e) => {
                        error!(error = %e, "failed to bind DoT listener");
                        process::exit(1);
                    }
                },
                Err(e) => {
                    log_tls_load_error(&e);
                    error!(error = %e, "failed to initialize DoT server");
                    process::exit(1);
                }
            }
        } else {
            info!("[server.tls].enabled is false — DoT server disabled");
        }
    }

    // Install the SIGHUP hot-reload handler (story 02-004).
    //
    // On SIGHUP, HotReloadManager reloads dnshub.toml from disk, validates it,
    // and atomically swaps the active config via ConfigStore (ArcSwap). If a
    // blocklist daemon notify is wired, it also triggers an immediate
    // blocklist refresh. Invalid configs are logged and discarded — the
    // server keeps serving with the last-known-good config.
    install_sighup_hot_reload(config_store.clone());

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

/// Install the SIGHUP hot-reload handler.
///
/// Resolves the config path from `DNSHUB_CONFIG` (default
/// `/etc/dnshub/dnshub.toml`) and the optional blocklists path from
/// `DNSHUB_BLOCKLISTS` (default `/etc/dnshub/blocklists.toml`). If the
/// config path does not exist on disk the hot-reload handler is still
/// installed — a SIGHUP will then log an error and keep the in-memory
/// config, which is the correct behavior for the default-config scaffold.
fn install_sighup_hot_reload(config_store: std::sync::Arc<ConfigStore>) {
    let config_path = std::env::var("DNSHUB_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/dnshub/dnshub.toml"));

    let blocklists_path = std::env::var("DNSHUB_BLOCKLISTS")
        .map(PathBuf::from)
        .ok();

    let paths = HotReloadPaths {
        config: config_path,
        blocklists: blocklists_path,
    };

    // The blocklist daemon notify is not wired here yet (the daemon is
    // instantiated in a later story that wires the full runtime). When the
    // daemon is available, pass its `refresh_notify()` as the third arg.
    let manager = std::sync::Arc::new(HotReloadManager::new(
        config_store,
        paths,
        None::<std::sync::Arc<tokio::sync::Notify>>,
    ));

    tokio::spawn(async move {
        manager.run().await;
    });

    info!("SIGHUP hot-reload handler scheduled");
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
