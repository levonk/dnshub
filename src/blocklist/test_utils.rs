//! Test utilities for the blocklist module.
//!
//! Provides a simple temp directory helper that does not depend on the
//! `tempfile` crate (which is only a transitive dependency of `heed` and
//! not declared in `[dev-dependencies]`).

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A simple temp directory that removes itself on drop.
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Create a new unique temp directory under the system temp dir.
    pub fn new() -> std::io::Result<Self> {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "dnshub-blocklist-test-{}-{}",
            std::process::id(),
            id
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    /// Returns the path to the temp directory.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
