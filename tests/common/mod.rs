//! Shared test utilities for hot-reload integration tests.
//!
//! Provides helpers to:
//! - Build a `ConfigStore` / `HotReloadManager` backed by real on-disk
//!   TOML config files in a unique temp directory.
//! - Build a `HotSwapStore` backed by real LMDB stores.
//! - Generate concurrent "query load" by spawning many tokio tasks that
//!   read the active config / blocklist snapshot (simulating in-flight
//!   DNS queries) while a reload or hot-swap happens concurrently.
//!
//! All helpers use unique temp directories (per process + atomic counter)
//! so parallel `cargo test` invocations never collide. The OS reclaims
//! `/tmp` so explicit cleanup is skipped (matching the convention in
//! `src/config/hot_reload.rs` and `src/blocklist/test_utils.rs`).

#![allow(dead_code)]

use dnshub::blocklist::{
    BlocklistCompiler, BlocklistEntry, BlocklistMetadata, BlocklistStore, HotSwapStore,
    LmdbBlocklistStore,
};
use dnshub::config::{
    ConfigStore, DnshubConfig, HotReloadManager, HotReloadPaths, UpstreamConfig,
};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Create a unique temp directory under the system temp dir.
///
/// Each call produces a distinct path so parallel tests never collide.
/// The directory is not automatically removed (the OS reclaims `/tmp`),
/// matching the convention used by the in-crate test helpers.
pub fn temp_dir(label: &str) -> PathBuf {
    let id = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("dnshub-hot-reload-it-{label}-{pid}-{id}"));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

/// Write a valid `dnshub.toml` to `path` with a single tier-1 upstream
/// whose address is `upstream_addr` and whose name is `name`.
pub fn write_valid_config(path: &Path, upstream_addr: &str, name: &str) {
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
    let mut f = std::fs::File::create(path).expect("create config file");
    f.write_all(toml.as_bytes()).expect("write config file");
}

/// Write an invalid `dnshub.toml` to `path` (no upstreams → validation
/// fails). Used to test that a failed reload keeps the previous config.
pub fn write_invalid_config(path: &Path) {
    let toml = r#"
[metrics]
listen = "0.0.0.0:9090"
path = "/metrics"

[logging]
level = "info"
format = "json"
"#;
    let mut f = std::fs::File::create(path).expect("create config file");
    f.write_all(toml.as_bytes()).expect("write config file");
}

/// Build a `ConfigStore` holding a single tier-1 upstream at
/// `upstream_addr`. This is the "initial" config used before a reload.
pub fn make_config_store(upstream_addr: &str, name: &str) -> Arc<ConfigStore> {
    let mut cfg = DnshubConfig::default();
    cfg.upstreams.push(UpstreamConfig {
        name: name.to_string(),
        address: upstream_addr.to_string(),
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

/// Build a `HotReloadManager` wired to `store` and the on-disk config at
/// `config_path`. When `blocklist_refresh` is `Some`, a successful reload
/// signals the provided `Notify` (simulating the blocklist daemon trigger).
pub fn make_hot_reload_manager(
    store: Arc<ConfigStore>,
    config_path: PathBuf,
    blocklist_refresh: Option<Arc<Notify>>,
) -> HotReloadManager {
    HotReloadManager::new(
        store,
        HotReloadPaths {
            config: config_path,
            blocklists: None,
        },
        blocklist_refresh,
    )
}

/// Build an LMDB `LmdbBlocklistStore` at `dir` containing `n` entries
/// of the form `domain{i}.{suffix}`.
pub fn make_lmdb_store(dir: &Path, n: usize, suffix: &str) -> LmdbBlocklistStore {
    let store = LmdbBlocklistStore::open(dir, None).expect("open lmdb store");
    let meta = BlocklistMetadata {
        categories: 0,
        sources: 1,
        first_seen: 1_700_000_000,
        last_updated: 1_700_000_000,
    };
    let entries: Vec<(String, BlocklistMetadata)> = (0..n)
        .map(|i| (format!("domain{i}.{suffix}"), meta))
        .collect();
    store.put_batch_owned(entries).expect("seed lmdb store");
    store
}

/// Build a `HotSwapStore` wrapping an LMDB store with `n` entries.
pub fn make_hot_swap_store(dir: &Path, n: usize, suffix: &str) -> Arc<HotSwapStore> {
    let live_path = dir.join("live.mdb");
    let store = make_lmdb_store(&dir.join("initial"), n, suffix);
    Arc::new(HotSwapStore::new(store, live_path))
}

/// Compile a fresh set of `entries` into a new LMDB store at a temp path
/// and return it. Used to simulate a blocklist refresh producing a new
/// database for hot-swap.
pub fn compile_store(dir: &Path, entries: Vec<BlocklistEntry>) -> LmdbBlocklistStore {
    let compiler = BlocklistCompiler::default();
    compiler
        .compile(&entries, dir, None::<&Path>)
        .expect("compile blocklist entries")
}

/// Result of a load-generation run: counts of successful and failed
/// "queries" (snapshot reads). A failure means a read returned an error
/// or a corrupted/inconsistent snapshot — under ArcSwap this should
/// never happen.
#[derive(Debug, Clone, Default)]
pub struct LoadResult {
    /// Number of snapshot reads that completed without error.
    pub success: u64,
    /// Number of snapshot reads that returned an error.
    pub failures: u64,
}

impl LoadResult {
    /// Merge another result into this one (sum the counters).
    pub fn merge(&mut self, other: &LoadResult) {
        self.success += other.success;
        self.failures += other.failures;
    }

    /// Returns `true` if there were zero failures.
    pub fn zero_failures(&self) -> bool {
        self.failures == 0
    }
}

/// A handle to a running load generator. The load is produced by `count`
/// tokio tasks, each performing `iterations` snapshot reads of the shared
/// `ConfigStore`. Dropping the join handles aborts the tasks.
pub struct LoadHandle {
    pub join: tokio::task::JoinHandle<LoadResult>,
}

/// Spawn `count` tokio tasks that each perform `iterations` config
/// snapshot reads from `store`. Each read loads a full `Arc<DnshubConfig>`
/// and verifies the upstream address is non-empty (simulating a query
/// consuming the config). Returns a `LoadHandle` for the aggregate result.
///
/// The tasks run concurrently with any reload triggered by the caller on
/// the same `ConfigStore`. Because `ConfigStore` is backed by `ArcSwap`,
/// every read must observe a consistent snapshot — no partial updates,
/// no panics, no errors.
pub fn spawn_config_load(
    store: Arc<ConfigStore>,
    count: usize,
    iterations: usize,
) -> Vec<LoadHandle> {
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let s = store.clone();
        let h = tokio::spawn(async move {
            let mut result = LoadResult::default();
            for _ in 0..iterations {
                // yield between reads so the scheduler interleaves with
                // any concurrent reload task.
                tokio::task::yield_now().await;
                let cfg = s.load_full();
                // A valid config always has at least one upstream with a
                // non-empty address. If we ever observe an empty address
                // (or an empty upstream list), the swap published a
                // partially-constructed config — an atomicity violation.
                let ok = cfg
                    .upstreams
                    .first()
                    .map(|u| !u.address.is_empty())
                    .unwrap_or(false);
                if ok {
                    result.success += 1;
                } else {
                    result.failures += 1;
                }
            }
            result
        });
        handles.push(LoadHandle { join: h });
    }
    handles
}

/// Await all load handles and merge their results into a single
/// `LoadResult`.
pub async fn collect_load(handles: Vec<LoadHandle>) -> LoadResult {
    let mut total = LoadResult::default();
    for h in handles {
        match h.join.await {
            Ok(r) => total.merge(&r),
            Err(_) => total.failures += 1,
        }
    }
    total
}

/// Spawn `count` tokio tasks that each perform `iterations` blocklist
/// lookups against `hot_swap`. Each lookup loads the active store via
/// `HotSwapStore::load` (an `ArcSwap` read) and checks a domain. Returns
/// join handles for the aggregate results.
///
/// The tasks run concurrently with any `swap` / `swap_database` triggered
/// by the caller on the same `HotSwapStore`. Because the swap is atomic,
/// every lookup must observe a consistent store — no corrupted reads,
/// no panics.
pub fn spawn_blocklist_load(
    hot_swap: Arc<HotSwapStore>,
    count: usize,
    iterations: usize,
    suffix: &str,
) -> Vec<LoadHandle> {
    let suffix = suffix.to_string();
    let mut handles = Vec::with_capacity(count);
    for _ in 0..count {
        let hs = hot_swap.clone();
        let suffix = suffix.clone();
        let h = tokio::spawn(async move {
            let mut result = LoadResult::default();
            for i in 0..iterations {
                tokio::task::yield_now().await;
                let store = hs.load();
                // Look up a domain that exists in one of the stores.
                let domain = format!("domain{}.{suffix}", i % 100);
                let lookup_result = store.lookup(&domain);
                match lookup_result {
                    Ok(_) => result.success += 1,
                    Err(_) => result.failures += 1,
                }
            }
            result
        });
        handles.push(LoadHandle { join: h });
    }
    handles
}
