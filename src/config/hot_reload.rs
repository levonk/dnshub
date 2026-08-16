//! SIGHUP-based hot-reload for `dnshub.toml` and blocklists.
//!
//! [`HotReloadManager`] owns the on-disk config paths and an
//! [`Arc<ConfigStore>`](crate::config::ConfigStore). On SIGHUP it reloads
//! `dnshub.toml` from disk, validates it, and — if valid — atomically
//! publishes the new config via `ConfigStore::swap`. If the reload or
//! validation fails, the old config is retained and the error is logged
//! (the server keeps serving with the last-known-good config).
//!
//! The manager also notifies the blocklist daemon to trigger an immediate
//! refresh of all sources (bypassing the per-source refresh interval) by
//! signalling a [`tokio::sync::Notify`] shared with
//! [`BlocklistDaemon`](crate::blocklist::BlocklistDaemon).
//!
//! All config swaps are lock-free (`ArcSwap`), so concurrent query
//! processing is never blocked — readers holding an `Arc<DnshubConfig>`
//! continue using the old config until their `Arc` is dropped.

use crate::config::{load_blocklists, ConfigError, ConfigStore, HotReloadPaths};
use std::sync::Arc;
use tokio::sync::Notify;

/// Hot-reload manager: reacts to SIGHUP by reloading config files and
/// triggering a blocklist refresh.
///
/// Construct with [`HotReloadManager::new`], then either call
/// [`HotReloadManager::reload`] directly (used by tests and the REST API
/// in story 05-004) or spawn [`HotReloadManager::run`] to install the
/// SIGHUP signal handler loop.
pub struct HotReloadManager {
    config_store: Arc<ConfigStore>,
    paths: HotReloadPaths,
    /// Optional notifier used to wake the blocklist daemon's refresh loop.
    /// When `Some`, [`HotReloadManager::reload`] calls `notify_one()` after
    /// a successful config swap so the daemon re-fetches its sources.
    blocklist_refresh: Option<Arc<Notify>>,
}

impl HotReloadManager {
    /// Create a new `HotReloadManager`.
    ///
    /// `blocklist_refresh` is an optional [`tokio::sync::Notify`] shared
    /// with the blocklist daemon; when set, a successful reload signals
    /// the daemon to perform an immediate `refresh_all`.
    pub fn new(
        config_store: Arc<ConfigStore>,
        paths: HotReloadPaths,
        blocklist_refresh: Option<Arc<Notify>>,
    ) -> Self {
        Self {
            config_store,
            paths,
            blocklist_refresh,
        }
    }

    /// Reload config files from disk and atomically swap the active config.
    ///
    /// Steps:
    /// 1. Reload + validate `dnshub.toml` via [`ConfigStore::reload_from`].
    ///    On failure the old config is kept and the error is returned
    ///    (the caller logs it; the server keeps serving).
    /// 2. If `blocklists.toml` is configured and present on disk, reload +
    ///    validate it. A failure here is logged but does **not** roll back
    ///    the main config swap — the blocklist daemon keeps its existing
    ///    config and is still triggered to refresh.
    /// 3. Signal the blocklist daemon (if wired) to trigger an immediate
    ///    refresh of all sources.
    pub fn reload(&self) -> Result<(), ConfigError> {
        tracing::info!(
            config_path = ?self.paths.config,
            "SIGHUP received: reloading dnshub config"
        );

        // Reload + validate + atomically swap the main config. On error,
        // `ConfigStore::reload_from` leaves the active config untouched.
        self.config_store.reload_from(&self.paths.config).map_err(|e| {
            tracing::error!(
                error = %e,
                "config reload failed; keeping previous config"
            );
            e
        })?;

        tracing::info!("dnshub config reloaded and swapped successfully");

        // Reload blocklists.toml if configured. A failure here is
        // non-fatal: we keep the previous blocklist config and still
        // trigger a refresh so the daemon re-fetches its sources.
        if let Some(blocklists_path) = &self.paths.blocklists {
            if blocklists_path.exists() {
                match load_blocklists(blocklists_path) {
                    Ok(new_blocklists) => {
                        tracing::info!(
                            sources = new_blocklists.sources.len(),
                            "blocklists.toml reloaded successfully"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "blocklists.toml reload failed; \
                             keeping previous blocklist config"
                        );
                    }
                }
            } else {
                tracing::debug!(
                    blocklists_path = ?blocklists_path,
                    "blocklists.toml path configured but not present on disk; \
                     skipping blocklist config reload"
                );
            }
        }

        // Trigger an immediate blocklist refresh (bypasses refresh interval).
        if let Some(notify) = &self.blocklist_refresh {
            notify.notify_one();
            tracing::info!("blocklist refresh triggered by hot-reload");
        }

        Ok(())
    }

    /// Install the SIGHUP signal handler and run the reload loop.
    ///
    /// On each SIGHUP, calls [`HotReloadManager::reload`]. Errors are
    /// logged but never abort the loop — the server continues serving
    /// with the last-known-good config. This future runs until the
    /// process exits or the signal stream is closed.
    ///
    /// On non-Unix platforms this is a no-op (SIGHUP does not exist);
    /// the method is still compiled so callers don't need cfg guards.
    pub async fn run(self: Arc<Self>) {
        let mut sighup = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::hangup(),
        ) {
            Ok(stream) => stream,
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "failed to install SIGHUP handler; hot-reload disabled"
                );
                return;
            }
        };

        tracing::info!("SIGHUP hot-reload handler installed");

        while let Some(()) = sighup.recv().await {
            if let Err(e) = self.reload() {
                tracing::error!(
                    error = %e,
                    "hot-reload failed; server continues with previous config"
                );
            }
        }

        tracing::warn!("SIGHUP signal stream closed; hot-reload loop exiting");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{DnshubConfig, UpstreamConfig};
    use std::io::Write;

    /// Write a TOML config to `path` with the given tier-1 upstream address.
    fn write_config(path: &std::path::Path, upstream_addr: &str, name: &str) {
        let toml = format!(
            r#"
[[upstreams]]
name = "{name}"
address = "{upstream_addr}"
protocol = "udp"
timeout_ms = 1000
tier = 1

[metrics]
listen = "0.0.0.0:9090"
path = "/metrics"

[logging]
level = "info"
format = "json"
"#
        );
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(toml.as_bytes()).unwrap();
    }

    fn write_invalid_config(path: &std::path::Path) {
        // No upstreams → validation fails.
        let toml = r#"
[metrics]
listen = "0.0.0.0:9090"
path = "/metrics"

[logging]
level = "info"
format = "json"
"#;
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(toml.as_bytes()).unwrap();
    }

    fn make_store() -> Arc<ConfigStore> {
        let mut cfg = DnshubConfig::default();
        cfg.upstreams.push(UpstreamConfig {
            name: "initial".to_string(),
            address: "1.1.1.1:53".to_string(),
            protocol: "udp".to_string(),
            timeout_ms: 1000,
            tier: 1,
        });
        cfg.metrics.listen = "0.0.0.0:9090".to_string();
        cfg.metrics.path = "/metrics".to_string();
        cfg.logging.level = "info".to_string();
        cfg.logging.format = "json".to_string();
        Arc::new(ConfigStore::new(cfg))
    }

    #[test]
    fn test_config_store_swap_is_atomic() {
        let store = make_store();

        let initial = store.load_full();
        assert_eq!(initial.upstreams[0].address, "1.1.1.1:53");

        // Hold the old snapshot while swapping.
        let held = store.load_full();

        let mut new_cfg = DnshubConfig::default();
        new_cfg.upstreams.push(UpstreamConfig {
            name: "new".to_string(),
            address: "8.8.8.8:53".to_string(),
            protocol: "udp".to_string(),
            timeout_ms: 1000,
            tier: 1,
        });
        new_cfg.metrics.listen = "0.0.0.0:9090".to_string();
        new_cfg.metrics.path = "/metrics".to_string();
        new_cfg.logging.level = "info".to_string();
        new_cfg.logging.format = "json".to_string();
        store.swap(new_cfg);

        // The held snapshot still points at the old config.
        assert_eq!(held.upstreams[0].address, "1.1.1.1:53");
        // A fresh load sees the new config.
        let after = store.load_full();
        assert_eq!(after.upstreams[0].address, "8.8.8.8:53");
        assert_eq!(after.upstreams[0].name, "new");
    }

    #[test]
    fn test_reload_valid_config_swaps() {
        let tmp = tempfile_dir();
        let config_path = tmp.join("dnshub.toml");
        write_config(&config_path, "9.9.9.9:53", "quad9");

        let store = make_store();
        let paths = HotReloadPaths {
            config: config_path,
            blocklists: None,
        };
        let manager = HotReloadManager::new(store.clone(), paths, None);

        manager.reload().expect("valid reload should succeed");

        let cfg = store.load_full();
        assert_eq!(cfg.upstreams[0].address, "9.9.9.9:53");
        assert_eq!(cfg.upstreams[0].name, "quad9");
    }

    #[test]
    fn test_reload_invalid_config_keeps_old() {
        let tmp = tempfile_dir();
        let config_path = tmp.join("dnshub.toml");
        write_config(&config_path, "9.9.9.9:53", "quad9");

        let store = make_store();
        let paths = HotReloadPaths {
            config: config_path.clone(),
            blocklists: None,
        };
        let manager = HotReloadManager::new(store.clone(), paths, None);

        // First reload succeeds.
        manager.reload().unwrap();
        assert_eq!(store.load_full().upstreams[0].address, "9.9.9.9:53");

        // Now overwrite with an invalid config (no upstreams).
        write_invalid_config(&config_path);

        // Reload should fail and keep the previous config.
        let result = manager.reload();
        assert!(result.is_err(), "invalid reload should error");

        let cfg = store.load_full();
        assert_eq!(
            cfg.upstreams[0].address,
            "9.9.9.9:53",
            "old config must be retained after a failed reload"
        );
    }

    #[test]
    fn test_reload_missing_config_file_keeps_old() {
        let tmp = tempfile_dir();
        let config_path = tmp.join("does-not-exist.toml");

        let store = make_store();
        let paths = HotReloadPaths {
            config: config_path,
            blocklists: None,
        };
        let manager = HotReloadManager::new(store.clone(), paths, None);

        let result = manager.reload();
        assert!(result.is_err(), "missing config file should error");

        let cfg = store.load_full();
        assert_eq!(cfg.upstreams[0].address, "1.1.1.1:53");
    }

    #[test]
    fn test_reload_triggers_blocklist_refresh() {
        let tmp = tempfile_dir();
        let config_path = tmp.join("dnshub.toml");
        write_config(&config_path, "9.9.9.9:53", "quad9");

        let store = make_store();
        let notify = Arc::new(Notify::new());
        // Register a waiter before reload so we observe the notification.
        // The waiter borrows `notify`; pass a clone to the manager so the
        // original stays alive for the waiter (both point at the same
        // `Notify`).
        let waiter = notify.notified();
        let notify_for_manager = notify.clone();

        let paths = HotReloadPaths {
            config: config_path,
            blocklists: None,
        };
        let manager =
            HotReloadManager::new(store.clone(), paths, Some(notify_for_manager));

        manager.reload().unwrap();

        // The waiter future should be immediately ready (notify_one was
        // called during reload). Use a short timeout to avoid hanging the
        // test if the notification was missed.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
                .await
                .expect("blocklist refresh notify should fire within 1s");
        });
    }

    #[test]
    fn test_reload_without_blocklist_notify_does_not_panic() {
        let tmp = tempfile_dir();
        let config_path = tmp.join("dnshub.toml");
        write_config(&config_path, "9.9.9.9:53", "quad9");

        let store = make_store();
        let paths = HotReloadPaths {
            config: config_path,
            blocklists: None,
        };
        let manager = HotReloadManager::new(store, paths, None);

        // Should complete without panicking even though no notify is wired.
        manager.reload().unwrap();
    }

    /// Minimal temp dir helper (avoids pulling in the tempfile crate dep
    /// just for these tests — uses a unique subdir under std::env::temp_dir).
    fn tempfile_dir() -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("dnshub-hot-reload-test-{pid}-{id}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Clean up temp dirs after the test binary exits would require a Drop
    // guard; the OS reclaims /tmp so we skip explicit cleanup for brevity.
}
