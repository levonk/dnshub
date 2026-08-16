//! Boot behaviour for the blocklist daemon.
//!
//! Implements the PRD section 4.3 boot strategy:
//!
//! 1. If a cached LMDB database exists on disk, load it and start serving
//!    immediately. A background refresh then updates the lists.
//! 2. If no cache exists, block startup until the **critical** sources
//!    (malware/phishing) have been fetched at least once. Non-critical
//!    sources are served as they arrive and do not block boot.
//!
//! The critical-source list is configurable (see
//! [`crate::blocklist::config::FailureHandlingConfig`]); the defaults are
//! `hagezi-tif` and `urlhaus`.

use std::path::Path;
use std::time::Duration;

use crate::blocklist::config::BlocklistsConfig;
use crate::blocklist::storage::LmdbBlocklistStore;
use crate::blocklist::Result;

/// Default boot timeout for waiting on critical sources (30 seconds).
pub const DEFAULT_BOOT_TIMEOUT: Duration = Duration::from_secs(30);

/// Outcome of a boot readiness check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootReadiness {
    /// A cached LMDB database was found on disk and can be served
    /// immediately while a background refresh runs.
    Cached,
    /// No cache exists; the daemon must block until critical sources load.
    ColdStart,
}

/// Determine boot readiness by checking for a cached LMDB database.
///
/// A cache is considered present when the live path directory exists and
/// contains at least one entry (i.e. a previous successful compilation
/// wrote it). The check is filesystem-only and does not open the
/// environment.
pub fn check_boot_readiness(live_path: &Path) -> BootReadiness {
    if has_cached_database(live_path) {
        BootReadiness::Cached
    } else {
        BootReadiness::ColdStart
    }
}

/// Returns `true` if `live_path` looks like a populated LMDB directory.
///
/// LMDB environments contain a `data.mdb` file (and usually a `lock.mdb`).
/// We treat the presence of `data.mdb` as the cache marker.
pub fn has_cached_database(live_path: &Path) -> bool {
    live_path.is_dir() && live_path.join("data.mdb").exists()
}

/// Attempt to load a cached LMDB store from `live_path`.
///
/// Returns `Ok(store)` if the cache exists and opens successfully, or
/// `Err` if the path is missing or cannot be opened. Callers should treat
/// an error as "no cache" and fall back to a cold start.
pub fn load_cached_store(live_path: &Path) -> Result<LmdbBlocklistStore> {
    if !has_cached_database(live_path) {
        return Err(crate::blocklist::BlocklistError::EnvOpen(format!(
            "no cached LMDB at {}",
            live_path.display()
        )));
    }
    LmdbBlocklistStore::open(live_path, None)
}

/// Returns the indices of the critical sources in the config's `sources`
/// list. A source is critical if its name appears in
/// `config.failure_handling.critical_sources`.
pub fn critical_source_indices(config: &BlocklistsConfig) -> Vec<usize> {
    let critical = &config.failure_handling.critical_sources;
    if critical.is_empty() {
        return Vec::new();
    }
    config
        .sources
        .iter()
        .enumerate()
        .filter(|(_, s)| critical.iter().any(|c| c == &s.name))
        .map(|(i, _)| i)
        .collect()
}

/// Returns `true` once all critical sources have been loaded at least once.
///
/// "Loaded" is determined by the per-source `loaded` flags passed in: the
/// daemon sets a flag to `true` after a successful fetch for that source.
/// Sources with no critical designation never block.
pub fn critical_sources_loaded(loaded: &[bool], critical_indices: &[usize]) -> bool {
    critical_indices.iter().all(|&i| loaded.get(i).copied().unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::config::{
        BlocklistsConfig, FailureHandlingConfig, Format, SourceConfig, StorageConfig,
    };
    use crate::blocklist::test_utils::TempDir;
    use crate::blocklist::BlocklistStore;

    fn make_config(sources: Vec<SourceConfig>, critical: Vec<String>) -> BlocklistsConfig {
        BlocklistsConfig {
            sources,
            storage: StorageConfig::default(),
            failure_handling: FailureHandlingConfig {
                critical_sources: critical,
                ..Default::default()
            },
        }
    }

    #[test]
    fn test_cold_start_when_no_cache() {
        let dir = TempDir::new().unwrap();
        let live = dir.path().join("does-not-exist");
        assert_eq!(check_boot_readiness(&live), BootReadiness::ColdStart);
        assert!(!has_cached_database(&live));
    }

    #[test]
    fn test_cached_when_data_mdb_present() {
        let dir = TempDir::new().unwrap();
        let live = dir.path().join("cache");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(live.join("data.mdb"), b"fake").unwrap();
        assert_eq!(check_boot_readiness(&live), BootReadiness::Cached);
        assert!(has_cached_database(&live));
    }

    #[test]
    fn test_load_cached_store_missing_returns_err() {
        let dir = TempDir::new().unwrap();
        let live = dir.path().join("missing");
        let result = load_cached_store(&live);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_cached_store_present_opens() {
        let dir = TempDir::new().unwrap();
        let live = dir.path().join("cache");
        // Create a real LMDB env so open succeeds.
        let store = LmdbBlocklistStore::open(&live, None).unwrap();
        drop(store);
        assert!(has_cached_database(&live));
        let loaded = load_cached_store(&live);
        assert!(loaded.is_ok(), "expected cached store to open");
        assert!(loaded.unwrap().is_empty().unwrap());
    }

    #[test]
    fn test_critical_source_indices() {
        let config = make_config(
            vec![
                SourceConfig {
                    name: "hagezi-tif".to_string(),
                    url: "http://x".to_string(),
                    format: Format::Domains,
                    categories: vec![],
                    refresh_hours: None,
                    refresh_minutes: None,
                },
                SourceConfig {
                    name: "urlhaus".to_string(),
                    url: "http://y".to_string(),
                    format: Format::Domains,
                    categories: vec![],
                    refresh_hours: None,
                    refresh_minutes: None,
                },
                SourceConfig {
                    name: "easylist".to_string(),
                    url: "http://z".to_string(),
                    format: Format::Domains,
                    categories: vec![],
                    refresh_hours: None,
                    refresh_minutes: None,
                },
            ],
            vec!["hagezi-tif".to_string(), "urlhaus".to_string()],
        );
        let idxs = critical_source_indices(&config);
        assert_eq!(idxs, vec![0, 1]);
    }

    #[test]
    fn test_critical_source_indices_empty_config() {
        let config = make_config(vec![], vec!["hagezi-tif".to_string()]);
        assert!(critical_source_indices(&config).is_empty());
    }

    #[test]
    fn test_critical_source_indices_no_critical() {
        let config = make_config(
            vec![SourceConfig {
                name: "easylist".to_string(),
                url: "http://z".to_string(),
                format: Format::Domains,
                categories: vec![],
                refresh_hours: None,
                refresh_minutes: None,
            }],
            vec![],
        );
        assert!(critical_source_indices(&config).is_empty());
    }

    #[test]
    fn test_critical_sources_loaded_all_true() {
        let loaded = vec![true, true, false];
        let critical = vec![0, 1];
        assert!(critical_sources_loaded(&loaded, &critical));
    }

    #[test]
    fn test_critical_sources_loaded_one_false() {
        let loaded = vec![true, false, true];
        let critical = vec![0, 1];
        assert!(!critical_sources_loaded(&loaded, &critical));
    }

    #[test]
    fn test_critical_sources_loaded_no_critical_is_true() {
        let loaded = vec![false, false];
        assert!(critical_sources_loaded(&loaded, &[]));
    }
}
