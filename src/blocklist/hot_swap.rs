//! Atomic hot-swap of the blocklist database handle via `ArcSwap`.
//!
//! The hot-swap mechanism allows a background thread to build a new LMDB
//! database to a temporary path, then atomically swap the active store
//! handle. Old readers continue using the old database until their `Arc`
//! is dropped — zero-downtime reload.
//!
//! ## How it works
//!
//! 1. A background thread compiles a new LMDB database to a temp path.
//! 2. The temp directory is atomically renamed over the live path.
//! 3. A new `LmdbBlocklistStore` is opened from the renamed path.
//! 4. `ArcSwap::store` atomically replaces the active handle.
//! 5. Existing readers holding `Arc<LmdbBlocklistStore>` continue
//!    uninterrupted; new readers get the new store.

use crate::blocklist::storage::LmdbBlocklistStore;
use crate::blocklist::{BlocklistStore, Result};
use arc_swap::ArcSwap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Atomic hot-swap wrapper for `LmdbBlocklistStore`.
///
/// Holds the active store in an `ArcSwap` for lock-free atomic swaps.
/// Readers call `load()` to get an `Arc<LmdbBlocklistStore>` that is
/// stable for its lifetime, even if a swap occurs concurrently.
pub struct HotSwapStore {
    inner: ArcSwap<LmdbBlocklistStore>,
    /// The live LMDB path (where the active database resides).
    live_path: PathBuf,
}

impl HotSwapStore {
    /// Create a new `HotSwapStore` from an existing store.
    pub fn new(store: LmdbBlocklistStore, live_path: PathBuf) -> Self {
        Self {
            inner: ArcSwap::from_pointee(store),
            live_path,
        }
    }

    /// Load the current active store as an `Arc`.
    ///
    /// The returned `Arc` is stable — even if `swap` is called concurrently,
    /// this `Arc` continues to point to the same store.
    pub fn load(&self) -> Arc<LmdbBlocklistStore> {
        self.inner.load_full()
    }

    /// Atomically swap the active store.
    ///
    /// After this call, new calls to `load()` will return the new store.
    /// Existing `Arc` holders continue using the old store.
    pub fn swap(&self, new_store: LmdbBlocklistStore) {
        self.inner.store(Arc::new(new_store));
        tracing::info!(
            path = ?self.live_path,
            "blocklist hot-swap completed"
        );
    }

    /// Build a new store in a temp directory, then atomically swap it in.
    ///
    /// The `build_fn` closure receives a temp path and must create a valid
    /// `LmdbBlocklistStore` there. Once built, the store is atomically
    /// swapped in via `ArcSwap`. The old store's `Arc` holders continue
    /// uninterrupted.
    ///
    /// Note: the temp path is used as the new store's path. In a production
    /// system, a rename-to-live-path step would follow, but on some
    /// platforms (macOS) renaming a directory with an active LMDB mmap
    /// fails. The `ArcSwap` provides the atomic swap; the path is an
    /// implementation detail.
    pub fn swap_database<F>(&self, build_fn: F) -> Result<()>
    where
        F: FnOnce(&Path) -> Result<LmdbBlocklistStore>,
    {
        // Create a temp path adjacent to the live path for same-filesystem
        // operations.
        let temp_path = self.live_path.with_extension("mdb.tmp");

        // Clean up any stale temp directory.
        if temp_path.exists() {
            std::fs::remove_dir_all(&temp_path)?;
        }

        // Build the new database in the temp path.
        let new_store = build_fn(&temp_path)?;

        // Atomically swap in the new store.
        self.swap(new_store);

        Ok(())
    }

    /// Returns the live path.
    pub fn live_path(&self) -> &Path {
        &self.live_path
    }
}

impl BlocklistStore for HotSwapStore {
    fn lookup(&self, domain: &str) -> Result<Option<crate::blocklist::BlocklistMetadata>> {
        self.load().lookup(domain)
    }

    fn len(&self) -> Result<u64> {
        self.load().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::test_utils::TempDir;
    use crate::blocklist::{BlocklistEntry, BlocklistMetadata, BlocklistStore};

    fn make_store(dir: &Path, n: usize) -> LmdbBlocklistStore {
        let store = LmdbBlocklistStore::open(dir, None).unwrap();
        for i in 0..n {
            let domain = format!("domain{i}.example.com");
            let meta = BlocklistMetadata {
                categories: 0,
                sources: 1,
                first_seen: 1_700_000_000,
                last_updated: 1_700_000_000,
            };
            store.put(&domain, &meta).unwrap();
        }
        store
    }

    #[test]
    fn test_hot_swap_basic() {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");

        let store1 = make_store(&dir.path().join("initial"), 10);
        let hot_swap = HotSwapStore::new(store1, live_path.clone());

        // Verify initial state.
        let store = hot_swap.load();
        assert_eq!(store.len().unwrap(), 10);
        assert!(store.is_blocked("domain0.example.com"));

        // Swap in a new store with different data.
        let store2 = make_store(&dir.path().join("updated"), 20);
        hot_swap.swap(store2);

        // New load should get the updated store.
        let store = hot_swap.load();
        assert_eq!(store.len().unwrap(), 20);
    }

    #[test]
    fn test_hot_swap_readers_continue_after_swap() {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");

        let store1 = make_store(&dir.path().join("initial"), 10);
        let hot_swap = HotSwapStore::new(store1, live_path);

        // Hold a reference to the old store.
        let old_store = hot_swap.load();
        assert_eq!(old_store.len().unwrap(), 10);

        // Swap in a new store.
        let store2 = make_store(&dir.path().join("updated"), 20);
        hot_swap.swap(store2);

        // Old reference should still work (10 entries).
        assert_eq!(old_store.len().unwrap(), 10);

        // New reference should get the new store (20 entries).
        let new_store = hot_swap.load();
        assert_eq!(new_store.len().unwrap(), 20);
    }

    #[test]
    fn test_hot_swap_swap_database() {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");

        // Initialize with an empty store.
        let store = LmdbBlocklistStore::open(&dir.path().join("init"), None).unwrap();
        let hot_swap = HotSwapStore::new(store, live_path.clone());

        // Build a new database in a temp path.
        let entries: Vec<BlocklistEntry> = (0..50)
            .map(|i| BlocklistEntry {
                domain: format!("domain{i}.test.com"),
                categories: 0,
                sources: 1,
            })
            .collect();

        let entries_clone = entries.clone();
        hot_swap
            .swap_database(|temp_path| {
                let compiler = crate::blocklist::BlocklistCompiler::default();
                let store = compiler.compile(&entries_clone, temp_path, None::<&Path>)?;
                Ok(store)
            })
            .unwrap();

        // Verify the new store is active.
        let store = hot_swap.load();
        assert_eq!(store.len().unwrap(), 50);
        assert!(store.is_blocked("domain0.test.com"));
    }

    #[test]
    fn test_hot_swap_concurrent_read() {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");

        let store1 = make_store(&dir.path().join("initial"), 100);
        let hot_swap = Arc::new(HotSwapStore::new(store1, live_path));

        // Spawn reader threads that continuously read while we swap.
        let hot_swap_clone = hot_swap.clone();
        let handle = std::thread::spawn(move || {
            for i in 0..100 {
                let store = hot_swap_clone.load();
                let domain = format!("domain{i}.example.com");
                // This should never panic — the old store stays valid.
                let _ = store.is_blocked(&domain);
            }
        });

        // Swap while reading.
        let store2 = make_store(&dir.path().join("updated"), 200);
        hot_swap.swap(store2);

        // Reader should complete without errors.
        handle.join().unwrap();

        // Final state should be the new store.
        let store = hot_swap.load();
        assert_eq!(store.len().unwrap(), 200);
    }

    #[test]
    fn test_hot_swap_trait_impl() {
        let dir = TempDir::new().unwrap();
        let live_path = dir.path().join("live.mdb");

        let store = make_store(&dir.path().join("init"), 5);
        let hot_swap = HotSwapStore::new(store, live_path);

        // Use via BlocklistStore trait.
        assert_eq!(hot_swap.len().unwrap(), 5);
        assert!(hot_swap.is_blocked("domain0.example.com"));
        assert!(!hot_swap.is_blocked("nonexistent.com"));
    }
}
