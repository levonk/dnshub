//! Async load generator for hot-reload integration tests.
//!
//! The generator spawns a configurable number of tokio tasks that each
//! perform snapshot reads of a shared `ConfigStore` (or blocklist
//! `HotSwapStore`) at a target rate for a target duration. It tracks
//! success / failure / timeout counts so the caller can verify that a
//! concurrent reload or hot-swap produced zero query failures.
//!
//! This is a *mock* load generator: instead of sending real UDP DNS
//! packets (which would require an upstream resolver and network
//! access), it exercises the same lock-free `ArcSwap` read path that
//! the DNS query handler uses to obtain the active config / blocklist
//! snapshot. This keeps the tests deterministic and network-independent
//! while still verifying the atomicity guarantees that matter for
//! hot-reload correctness.

mod common;

use common::{collect_load, spawn_blocklist_load, spawn_config_load, LoadHandle, LoadResult};
use dnshub::blocklist::HotSwapStore;
use dnshub::config::ConfigStore;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for a load-generation run.
#[derive(Debug, Clone)]
pub struct LoadConfig {
    /// Number of concurrent worker tasks to spawn.
    pub workers: usize,
    /// Number of snapshot reads each worker performs.
    pub iterations: usize,
    /// Optional delay between reads (simulates a query rate cap).
    /// When `None`, workers read as fast as possible.
    pub per_read_delay: Option<Duration>,
}

impl Default for LoadConfig {
    fn default() -> Self {
        Self {
            workers: 8,
            iterations: 1000,
            per_read_delay: None,
        }
    }
}

/// A load generator that runs config snapshot reads against a shared
/// `ConfigStore`.
pub struct ConfigLoadGenerator {
    store: Arc<ConfigStore>,
    config: LoadConfig,
}

impl ConfigLoadGenerator {
    /// Create a new generator targeting `store` with the given `config`.
    pub fn new(store: Arc<ConfigStore>, config: LoadConfig) -> Self {
        Self { store, config }
    }

    /// Spawn the worker tasks and return their join handles.
    pub fn spawn(&self) -> Vec<LoadHandle> {
        spawn_config_load(
            self.store.clone(),
            self.config.workers,
            self.config.iterations,
        )
    }

    /// Spawn, await, and return the aggregate `LoadResult`.
    pub async fn run(&self) -> LoadResult {
        let handles = self.spawn();
        collect_load(handles).await
    }
}

/// A load generator that runs blocklist lookups against a shared
/// `HotSwapStore`.
pub struct BlocklistLoadGenerator {
    hot_swap: Arc<HotSwapStore>,
    config: LoadConfig,
    suffix: String,
}

impl BlocklistLoadGenerator {
    /// Create a new generator targeting `hot_swap` with the given
    /// `config`. `suffix` is the domain suffix used for lookups (e.g.
    /// `"example.com"`).
    pub fn new(
        hot_swap: Arc<HotSwapStore>,
        config: LoadConfig,
        suffix: impl Into<String>,
    ) -> Self {
        Self {
            hot_swap,
            config,
            suffix: suffix.into(),
        }
    }

    /// Spawn the worker tasks and return their join handles.
    pub fn spawn(&self) -> Vec<LoadHandle> {
        spawn_blocklist_load(
            self.hot_swap.clone(),
            self.config.workers,
            self.config.iterations,
            &self.suffix,
        )
    }

    /// Spawn, await, and return the aggregate `LoadResult`.
    pub async fn run(&self) -> LoadResult {
        let handles = self.spawn();
        collect_load(handles).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::make_config_store;

    /// Smoke test: the load generator completes without errors when
    /// there is no concurrent reload.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn config_load_generator_smoke() {
        let store = make_config_store("1.1.1.1:53", "initial");
        let gen = ConfigLoadGenerator::new(
            store,
            LoadConfig {
                workers: 4,
                iterations: 100,
                per_read_delay: None,
            },
        );
        let result = gen.run().await;
        assert!(result.zero_failures(), "no failures without reload");
        assert_eq!(result.success, 400);
    }
}
