//! dnshub binary entry point.
//!
//! Loads configuration, builds the handler chain (Caching → Tiered Forwarding
//! via the `TieredForwardHandler` installed in the `Catalog`), starts the
//! hickory-server `Server` on UDP+TCP :53, optionally starts DoT (:853) and
//! the custom DoH (:443) server, spawns the REST API server, initializes
//! observability (JSON logging + Jaeger OTLP tracing), wires the blocklist
//! daemon refresh notification into the SIGHUP hot-reload handler, attaches
//! the query logger, and handles graceful shutdown via Ctrl+C.

use dnshub::api::{ApiServer, ApiServerOptions, AppState};
use dnshub::blocklist::config::BlocklistsConfig;
use dnshub::blocklist::daemon::BlocklistDaemon;
use dnshub::blocklist::hot_swap::HotSwapStore;
use dnshub::blocklist::storage::LmdbBlocklistStore;
use dnshub::config::{
    ConfigStore, DnshubConfig, HotReloadManager, HotReloadPaths,
};
use dnshub::dns::doh_axum::DohAxumServer;
use dnshub::dns::dot::{DotServer, log_tls_load_error};
use dnshub::dns::forwarding::ForwardingHandler;
use dnshub::dns::server::DnshubServer;
use dnshub::dns::DnshubHandler;
use dnshub::metrics::recorder::init_recorder as init_metrics_recorder;
use dnshub::observability::init_observability;
use dnshub::query_log::{QueryLogger, QueryLogRetention};
use hickory_server::zone_handler::Catalog;
use std::path::PathBuf;
use std::process;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    // Load configuration early so we can initialize observability with
    // the configured log level, format, and tracing endpoint.
    let config = load_config();

    // Initialize observability (JSON logging + Jaeger OTLP tracing).
    // This installs the global tracing subscriber. If it fails (e.g.
    // because the subscriber is already set), we log to stderr and
    // continue with the default subscriber.
    if let Err(e) = init_observability(&config) {
        eprintln!("warning: observability init failed: {e}");
    }

    info!(version = dnshub::VERSION, "starting dnshub");
    info!(upstreams = config.upstreams.len(), "configuration loaded");

    // Wrap the config in a ConfigStore (ArcSwap) so the SIGHUP hot-reload
    // handler can atomically swap in a reloaded config without blocking
    // concurrent query processing.
    let config_store = Arc::new(ConfigStore::new(config));

    // Initialize the Prometheus metrics recorder (no HTTP listener; the
    // API server exposes /metrics via the handle). If a global recorder
    // is already installed (e.g. by a test), this fails and we continue.
    let metrics_handle = match init_metrics_recorder() {
        Ok(h) => Some(h),
        Err(e) => {
            warn!(error = %e, "metrics recorder already installed or failed to init");
            None
        }
    };

    // Initialize the query logger when [query_log].enabled is true.
    let query_logger = init_query_logger(&config_store.load_full());

    // Build the handler chain with tiered forwarding.
    let handler = match build_handler(&config_store.load_full(), query_logger.clone()) {
        Ok(h) => h,
        Err(e) => {
            error!(error = %e, "failed to build DNS handler");
            process::exit(1);
        }
    };

    // The DoH server needs its own handler instance (Catalog is not Clone).
    // We build a second handler for DoH when DoH is enabled, sharing the
    // same query logger.
    let config_snapshot = config_store.load_full();
    let doh_handler = if let Some(doh_cfg) = &config_snapshot.server.doh {
        if doh_cfg.enabled {
            match build_handler(&config_snapshot, query_logger.clone()) {
                Ok(h) => Some(Arc::new(h)),
                Err(e) => {
                    error!(error = %e, "failed to build DoH handler");
                    process::exit(1);
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Start the server on the configured listen addresses.
    let mut server = DnshubServer::new(handler);
    for addr in &config_snapshot.server.listen {
        // Register UDP first (the primary DNS transport).
        if let Err(e) = server.register_udp(addr, &config_snapshot.server).await {
            error!(addr = %addr, error = %e, "failed to bind UDP listener");
            process::exit(1);
        }
        // Then TCP (for large responses / zone transfers).
        if let Err(e) = server.register_tcp(addr, &config_snapshot.server).await {
            error!(addr = %addr, error = %e, "failed to bind TCP listener");
            process::exit(1);
        }
    }

    // Start the DoT (DNS-over-TLS) server on :853 when [server.tls].enabled.
    if let Some(tls) = &config_snapshot.server.tls {
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

    // Start the custom DoH (DNS-over-HTTPS) server when [server.doh].enabled.
    // The custom server supports both POST (wire-format body) and GET
    // (?dns= base64url parameter) per RFC 8484, unlike hickory-server 0.26's
    // built-in HTTPS listener which only handles POST.
    if let Some(doh_cfg) = &config_snapshot.server.doh {
        if doh_cfg.enabled {
            let handler_arc = doh_handler
                .clone()
                .expect("DoH handler should have been built when DoH is enabled");
            match DohAxumServer::new(doh_cfg, config_snapshot.server.tls.as_ref(), handler_arc) {
                Ok(doh) => {
                    let doh = Arc::new(doh);
                    let addrs = doh.serve_all().await;
                    for a in &addrs {
                        info!(addr = %a, "custom DoH server listening (GET + POST)");
                    }
                }
                Err(e) => {
                    error!(error = %e, "failed to build custom DoH server");
                    process::exit(1);
                }
            }
        } else {
            info!("[server.doh].enabled is false — DoH server disabled");
        }
    }

    // Initialize and start the blocklist daemon when blocklists.toml exists.
    // The daemon's refresh Notify is shared with the SIGHUP hot-reload
    // handler so a config reload triggers an immediate blocklist refresh.
    let blocklist_notify = init_blocklist_daemon(&config_store.load_full()).await;

    // Spawn the REST API server when [api].enabled is true.
    spawn_api_server(
        &config_store,
        metrics_handle,
        blocklist_notify.clone(),
        &config_store.load_full(),
    );

    // Install the SIGHUP hot-reload handler with the blocklist notify.
    install_sighup_hot_reload(config_store.clone(), blocklist_notify);

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
/// Reads `/etc/dnshub/dnshub.toml` (overridable via `DNSHUB_CONFIG`), validates
/// it, and falls back to built-in defaults when the file is missing or
/// invalid (logging a warning).
fn load_config() -> DnshubConfig {
    let config_path = std::env::var("DNSHUB_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/dnshub/dnshub.toml"));

    if config_path.exists() {
        match dnshub::config::load_config(&config_path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "warning: failed to load config from {}: {} — using defaults",
                    config_path.display(),
                    e
                );
                DnshubConfig::defaults()
            }
        }
    } else {
        info!(
            path = %config_path.display(),
            "config file not found, using built-in defaults"
        );
        DnshubConfig::defaults()
    }
}

/// Initialize the query logger when `[query_log].enabled` is true.
///
/// Returns `Some(Arc<QueryLogger>)` when enabled, `None` otherwise.
/// The logger opens a SQLite database at `/var/lib/dnshub/query-log.sqlite`
/// (overridable via `DNSHUB_QUERY_LOG_DB`). On failure, a warning is logged
/// and `None` is returned (the server continues without query logging).
fn init_query_logger(config: &DnshubConfig) -> Option<Arc<QueryLogger>> {
    if !config.query_log.enabled {
        info!("[query_log].enabled is false — query logging disabled");
        return None;
    }

    let db_path = std::env::var("DNSHUB_QUERY_LOG_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/dnshub/query-log.sqlite"));

    let retention = QueryLogRetention::from_config(&config.query_log);
    match QueryLogger::open(&db_path, retention) {
        Ok(logger) => {
            info!(
                db = %db_path.display(),
                max_entries = config.query_log.max_entries,
                retention_days = config.query_log.retention_days,
                "query logger initialized",
            );
            // Spawn a background cleanup task that runs periodically.
            let logger_arc = Arc::new(logger);
            let logger_for_cleanup = logger_arc.clone();
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
                interval.tick().await; // skip the immediate first tick
                loop {
                    interval.tick().await;
                    if let Err(e) = logger_for_cleanup.run_cleanup() {
                        warn!(error = %e, "query log cleanup failed");
                    }
                }
            });
            Some(logger_arc)
        }
        Err(e) => {
            warn!(error = %e, db = %db_path.display(), "failed to open query log database");
            None
        }
    }
}

/// Initialize and start the blocklist daemon when `blocklists.toml` exists.
///
/// Returns the `Arc<Notify>` that the SIGHUP hot-reload handler uses to
/// trigger immediate blocklist refreshes. When blocklists are not
/// configured, returns a standalone `Notify` (so the hot-reload handler
/// still has a valid handle to call `notify_one()` on).
async fn init_blocklist_daemon(_config: &DnshubConfig) -> Arc<tokio::sync::Notify> {
    let blocklists_path = std::env::var("DNSHUB_BLOCKLISTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/etc/dnshub/blocklists.toml"));

    if !blocklists_path.exists() {
        info!(
            path = %blocklists_path.display(),
            "blocklists.toml not found — blocklist daemon disabled"
        );
        return Arc::new(tokio::sync::Notify::new());
    }

    // Load blocklists.toml using the blocklist module's BlocklistsConfig
    // (which includes failure_handling, unlike the config module's version).
    let blocklists_config = match std::fs::read_to_string(&blocklists_path) {
        Ok(contents) => match toml::from_str::<BlocklistsConfig>(&contents) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "failed to parse blocklists.toml — daemon disabled");
                return Arc::new(tokio::sync::Notify::new());
            }
        },
        Err(e) => {
            warn!(error = %e, "failed to read blocklists.toml — daemon disabled");
            return Arc::new(tokio::sync::Notify::new());
        }
    };

    // Open the LMDB blocklist store.
    let lmdb_path = std::env::var("DNSHUB_BLOCKLIST_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/var/lib/dnshub/blocklist.lmdb"));

    let store = match LmdbBlocklistStore::open(&lmdb_path, None) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "failed to open blocklist LMDB store — daemon disabled");
            return Arc::new(tokio::sync::Notify::new());
        }
    };

    let hot_swap = HotSwapStore::new(store, lmdb_path.clone());

    let daemon = BlocklistDaemon::new(blocklists_config, Arc::new(hot_swap));

    // Get the refresh notify handle before spawning the daemon.
    let notify = daemon.refresh_notify();

    // Spawn the daemon's run loop in the background.
    let daemon_arc = Arc::new(daemon);
    tokio::spawn(async move {
        if let Err(e) = daemon_arc.run().await {
            error!(error = %e, "blocklist daemon exited with error");
        }
    });

    info!("blocklist daemon started");
    notify
}

/// Spawn the REST API server when `[api].enabled` is true.
///
/// The API server is configured with:
/// - Optional bearer-token auth (from `[api].auth_token`).
/// - Optional frontend static file serving (from `[frontend].static_dir`
///   when `[frontend].enabled` is true).
/// - The blocklist refresh notify handle (so `/api/v1/blocklists/refresh`
///   can trigger an immediate refresh).
fn spawn_api_server(
    config_store: &Arc<ConfigStore>,
    metrics_handle: Option<dnshub::metrics::recorder::PrometheusHandle>,
    blocklist_notify: Arc<tokio::sync::Notify>,
    config: &DnshubConfig,
) {
    if !config.api.enabled {
        info!("[api].enabled is false — REST API server disabled");
        return;
    }

    // Build the AppState with the config store, metrics handle, and
    // blocklist notify.
    let mut builder = AppState::builder(config_store.clone())
        .blocklist_refresh(blocklist_notify);

    if let Some(handle) = metrics_handle {
        builder = builder.metrics_handle(Arc::new(handle));
    }

    let state = Arc::new(builder.build());

    // Determine frontend static dir.
    let frontend_static_dir = if config.frontend.enabled && !config.frontend.static_dir.is_empty() {
        let path = PathBuf::from(&config.frontend.static_dir);
        if path.exists() {
            info!(static_dir = %path.display(), "frontend static file serving enabled");
            Some(path)
        } else {
            warn!(static_dir = %path.display(), "frontend static_dir does not exist — static serving disabled");
            None
        }
    } else {
        None
    };

    // Build the API server with auth + frontend options.
    let api_server = ApiServer::with_options(
        state,
        ApiServerOptions {
            auth_token: config.api.auth_token.clone(),
            frontend_static_dir,
        },
    );

    // Parse the listen address.
    let listen: std::net::SocketAddr = match config.api.listen.parse() {
        Ok(a) => a,
        Err(e) => {
            error!(addr = %config.api.listen, error = %e, "failed to parse API listen address");
            return;
        }
    };

    info!(addr = %listen, "spawning REST API server");
    tokio::spawn(async move {
        api_server.serve(listen).await;
    });
}

/// Install the SIGHUP hot-reload handler.
///
/// Resolves the config path from `DNSHUB_CONFIG` (default
/// `/etc/dnshub/dnshub.toml`) and the optional blocklists path from
/// `DNSHUB_BLOCKLISTS` (default `/etc/dnshub/blocklists.toml`). If the
/// config path does not exist on disk the hot-reload handler is still
/// installed — a SIGHUP will then log an error and keep the in-memory
/// config, which is the correct behavior for the default-config scaffold.
fn install_sighup_hot_reload(
    config_store: Arc<ConfigStore>,
    blocklist_notify: Arc<tokio::sync::Notify>,
) {
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

    let manager = Arc::new(HotReloadManager::new(
        config_store,
        paths,
        Some(blocklist_notify),
    ));

    tokio::spawn(async move {
        manager.run().await;
    });

    info!("SIGHUP hot-reload handler scheduled");
}

/// Build the [`DnshubHandler`] from the configuration: a [`Catalog`] with
/// tiered forwarding at the root, wrapped with the middleware chain and
/// optional query logger.
fn build_handler(
    config: &DnshubConfig,
    query_logger: Option<Arc<QueryLogger>>,
) -> Result<DnshubHandler, String> {
    let catalog = Catalog::new();
    let catalog = ForwardingHandler::install_tiered(catalog, &config.upstreams, &config.cache)?;

    let mut handler = DnshubHandler::from_catalog(catalog);

    // Attach the query logger when available.
    if let Some(logger) = query_logger {
        handler = handler.with_query_logger(logger);
    }

    Ok(handler)
}
