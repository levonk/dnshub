//! LMDB blocklist storage via the [`heed`] crate.
//!
//! Keys are reversed domain names (see [`crate::blocklist::reverse_domain`])
//! so that all subdomains of a blocked domain sort contiguously. Values are
//! 14-byte serialized [`BlocklistMetadata`] structs.
//!
//! ## Two-stage lookup
//!
//! [`LmdbBlocklistStore`] optionally holds a [`BloomIndex`] for the fast
//! negative path. When a Bloom filter is present, `lookup` first checks the
//! Bloom filter; if it returns `false`, the domain is definitely not blocked
//! and `None` is returned without touching LMDB. If the Bloom filter returns
//! `true` (maybe blocked), the LMDB database is consulted for confirmation
//! and metadata retrieval.

use crate::blocklist::bloom::BloomIndex;
use crate::blocklist::{
    reverse_domain, BlocklistError, BlocklistMetadata, BlocklistStore, CategoryBitmap, Result,
};
use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvOpenOptions};
use std::path::Path;

/// LMDB database name for domain entries.
const DB_NAME: &str = "domains";

/// Default mmap size: 256 MB. LMDB grows the file as needed up to this limit.
const DEFAULT_MAP_SIZE: usize = 256 * 1024 * 1024;

/// LMDB-backed blocklist store with optional Bloom filter front-end.
pub struct LmdbBlocklistStore {
    env: Env,
    db: Database<Str, Bytes>,
    bloom: Option<BloomIndex>,
}

impl LmdbBlocklistStore {
    /// Open (or create) an LMDB blocklist store at the given directory path.
    ///
    /// The directory must exist or be creatable. If a `bloom` filter is
    /// provided, two-stage lookup is enabled.
    pub fn open<P: AsRef<Path>>(path: P, bloom: Option<BloomIndex>) -> Result<Self> {
        Self::open_with_map_size(path, DEFAULT_MAP_SIZE, bloom)
    }

    /// Open with a custom mmap size (useful for tests with small databases).
    pub fn open_with_map_size<P: AsRef<Path>>(
        path: P,
        map_size: usize,
        bloom: Option<BloomIndex>,
    ) -> Result<Self> {
        let path = path.as_ref();
        std::fs::create_dir_all(path).map_err(|e| BlocklistError::EnvOpen(e.to_string()))?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(map_size)
                .max_dbs(8)
                .open(path)
                .map_err(|e| BlocklistError::EnvOpen(e.to_string()))?
        };

        let mut wtxn = env.write_txn()?;
        let db: Database<Str, Bytes> = env.create_database(&mut wtxn, Some(DB_NAME))?;
        wtxn.commit()?;

        Ok(Self { env, db, bloom })
    }

    /// Returns a reference to the Bloom filter, if present.
    pub fn bloom(&self) -> Option<&BloomIndex> {
        self.bloom.as_ref()
    }

    /// Returns a clone of the underlying heed `Env`.
    ///
    /// The `Env` is internally `Arc`-based so cloning is cheap.
    pub fn env(&self) -> Env {
        self.env.clone()
    }

    /// Returns the underlying heed `Database` handle.
    pub fn database(&self) -> Database<Str, Bytes> {
        self.db
    }

    /// Insert a single domain with metadata.
    ///
    /// The domain is reversed before storage (`www.example.com` →
    /// `com.example.www`).
    pub fn put(&self, domain: &str, metadata: &BlocklistMetadata) -> Result<()> {
        let key = reverse_domain(domain);
        let value = metadata.to_bytes();
        let mut wtxn = self.env.write_txn()?;
        self.db.put(&mut wtxn, &key, value.as_slice())?;
        wtxn.commit()?;
        Ok(())
    }

    /// Insert multiple entries in a single transaction.
    ///
    /// Each entry's domain is reversed and stored with its metadata.
    /// This is much more efficient than calling `put` in a loop.
    pub fn put_batch<'a, I>(&self, entries: I) -> Result<()>
    where
        I: IntoIterator<Item = (&'a str, &'a BlocklistMetadata)>,
    {
        let mut wtxn = self.env.write_txn()?;
        for (domain, metadata) in entries {
            let key = reverse_domain(domain);
            let value = metadata.to_bytes();
            self.db.put(&mut wtxn, &key, value.as_slice())?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Insert multiple owned entries in a single transaction.
    ///
    /// Like `put_batch` but takes owned `(String, BlocklistMetadata)` tuples,
    /// which is more convenient when metadata is constructed inline.
    pub fn put_batch_owned(&self, entries: Vec<(String, BlocklistMetadata)>) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        for (domain, metadata) in &entries {
            let key = reverse_domain(domain);
            let value = metadata.to_bytes();
            self.db.put(&mut wtxn, &key, value.as_slice())?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Retrieve metadata for a specific domain (forward domain, e.g.
    /// `www.example.com`). Performs LMDB lookup only (no Bloom filter).
    pub fn get(&self, domain: &str) -> Result<Option<BlocklistMetadata>> {
        let key = reverse_domain(domain);
        let rtxn = self.env.read_txn()?;
        match self.db.get(&rtxn, &key)? {
            Some(bytes) => Ok(Some(BlocklistMetadata::from_bytes(bytes)?)),
            None => Ok(None),
        }
    }

    /// Retrieve the category bitmap for a domain.
    ///
    /// Returns `Some(bitmap)` if the domain is stored, or `None` if the
    /// domain is not in the blocklist. This is a convenience wrapper around
    /// [`get`](Self::get) that extracts just the `categories` field.
    pub fn get_categories(&self, domain: &str) -> Result<Option<CategoryBitmap>> {
        Ok(self.get(domain)?.map(|meta| meta.categories))
    }

    /// Delete a single domain entry.
    pub fn delete(&self, domain: &str) -> Result<bool> {
        let key = reverse_domain(domain);
        let mut wtxn = self.env.write_txn()?;
        let deleted = self.db.delete(&mut wtxn, &key)?;
        wtxn.commit()?;
        Ok(deleted)
    }

    /// Clear all entries from the database.
    pub fn clear(&self) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        self.db.clear(&mut wtxn)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Iterate over all (reversed key, metadata) pairs in key order.
    ///
    /// The closure receives each entry and may return `false` to stop
    /// iteration.
    pub fn for_each<F>(&self, mut f: F) -> Result<()>
    where
        F: FnMut(&str, &BlocklistMetadata) -> bool,
    {
        let rtxn = self.env.read_txn()?;
        let iter = self.db.iter(&rtxn)?;
        for item in iter {
            let (key, value) = item?;
            let metadata = BlocklistMetadata::from_bytes(value)?;
            if !f(key, &metadata) {
                break;
            }
        }
        Ok(())
    }

    /// Iterate over all (reversed key, metadata) pairs in key order,
    /// returning a `Vec`. Useful for building Bloom filters from existing
    /// LMDB contents.
    pub fn entries(&self) -> Result<Vec<(String, BlocklistMetadata)>> {
        let rtxn = self.env.read_txn()?;
        let iter = self.db.iter(&rtxn)?;
        let mut out = Vec::new();
        for item in iter {
            let (key, value) = item?;
            let metadata = BlocklistMetadata::from_bytes(value)?;
            out.push((key.to_string(), metadata));
        }
        Ok(out)
    }

    /// Collect all reversed keys (for Bloom filter rebuilding).
    pub fn keys(&self) -> Result<Vec<String>> {
        let rtxn = self.env.read_txn()?;
        let iter = self.db.iter(&rtxn)?;
        let mut out = Vec::new();
        for item in iter {
            let (key, _) = item?;
            out.push(key.to_string());
        }
        Ok(out)
    }

    /// Prefix search: return all entries whose reversed key starts with the
    /// given prefix.
    ///
    /// For example, `prefix_search("com.example.")` returns all entries for
    /// `example.com` and its subdomains.
    pub fn prefix_search(&self, prefix: &str) -> Result<Vec<(String, BlocklistMetadata)>> {
        let rtxn = self.env.read_txn()?;
        let iter = self.db.prefix_iter(&rtxn, prefix)?;
        let mut out = Vec::new();
        for item in iter {
            let (key, value) = item?;
            let metadata = BlocklistMetadata::from_bytes(value)?;
            out.push((key.to_string(), metadata));
        }
        Ok(out)
    }

    /// Look up a domain using the two-stage Bloom + LMDB path.
    ///
    /// If a Bloom filter is present and returns `false`, the domain is
    /// definitely not blocked — return `None` immediately. Otherwise, fall
    /// through to LMDB.
    fn lookup_internal(&self, domain: &str) -> Result<Option<BlocklistMetadata>> {
        // Stage 1: Bloom filter (fast negative path).
        if let Some(bloom) = &self.bloom {
            let reversed = reverse_domain(domain);
            if !bloom.contains(&reversed) {
                return Ok(None);
            }
        }
        // Stage 2: LMDB confirmation.
        self.get(domain)
    }

}

impl BlocklistStore for LmdbBlocklistStore {
    fn lookup(&self, domain: &str) -> Result<Option<BlocklistMetadata>> {
        self.lookup_internal(domain)
    }

    fn len(&self) -> Result<u64> {
        let rtxn = self.env.read_txn()?;
        Ok(self.db.len(&rtxn)?)
    }
}

// SAFETY: `Env` is `Send + Sync` (heed marks it so). `Database` is `Copy`
// and `Send + Sync`. `BloomIndex` is `Send + Sync` (wraps a `fastbloom::
// BloomFilter` which is `Send + Sync`). Therefore the whole struct is
// `Send + Sync`.
unsafe impl Send for LmdbBlocklistStore {}
unsafe impl Sync for LmdbBlocklistStore {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::test_utils::TempDir;

    fn make_meta(categories: u32, sources: u16) -> BlocklistMetadata {
        BlocklistMetadata {
            categories,
            sources,
            first_seen: 1_700_000_000,
            last_updated: 1_700_000_000,
        }
    }

    #[test]
    fn test_store_put_and_get() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        let meta = make_meta(1, 1);
        store.put("www.example.com", &meta).unwrap();

        let retrieved = store.get("www.example.com").unwrap();
        assert_eq!(retrieved, Some(meta));

        // Non-existent domain.
        assert_eq!(store.get("nonexistent.com").unwrap(), None);
    }

    #[test]
    fn test_store_put_batch() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        let meta1 = make_meta(1, 1);
        let meta2 = make_meta(2, 1);
        let meta3 = make_meta(3, 1);

        store
            .put_batch([
                ("ads.example.com", &meta1),
                ("tracker.example.com", &meta2),
                ("malware.example.com", &meta3),
            ])
            .unwrap();

        assert_eq!(store.len().unwrap(), 3);
        assert_eq!(store.get("ads.example.com").unwrap(), Some(meta1));
        assert_eq!(store.get("tracker.example.com").unwrap(), Some(meta2));
        assert_eq!(store.get("malware.example.com").unwrap(), Some(meta3));
    }

    #[test]
    fn test_store_reversed_key_ordering() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        // Insert in forward order; keys should be reversed.
        store.put("www.example.com", &make_meta(1, 1)).unwrap();
        store.put("ads.example.com", &make_meta(2, 1)).unwrap();
        store.put("example.com", &make_meta(3, 1)).unwrap();

        let entries = store.entries().unwrap();
        // Reversed keys: com.example, com.example.ads, com.example.www
        assert_eq!(entries[0].0, "com.example");
        assert_eq!(entries[1].0, "com.example.ads");
        assert_eq!(entries[2].0, "com.example.www");
    }

    #[test]
    fn test_store_prefix_search() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        store.put("example.com", &make_meta(1, 1)).unwrap();
        store.put("www.example.com", &make_meta(2, 1)).unwrap();
        store.put("ads.example.com", &make_meta(3, 1)).unwrap();
        store.put("other.com", &make_meta(4, 1)).unwrap();

        // Prefix search for all example.com subdomains.
        let results = store.prefix_search("com.example.").unwrap();
        assert_eq!(results.len(), 2); // www and ads, but not example.com itself

        // Prefix search including example.com itself.
        let results = store.prefix_search("com.example").unwrap();
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_store_delete() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        store.put("example.com", &make_meta(1, 1)).unwrap();
        assert!(store.get("example.com").unwrap().is_some());

        assert!(store.delete("example.com").unwrap());
        assert!(store.get("example.com").unwrap().is_none());

        // Deleting again returns false.
        assert!(!store.delete("example.com").unwrap());
    }

    #[test]
    fn test_store_clear() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        store.put("a.com", &make_meta(1, 1)).unwrap();
        store.put("b.com", &make_meta(2, 1)).unwrap();
        assert_eq!(store.len().unwrap(), 2);

        store.clear().unwrap();
        assert_eq!(store.len().unwrap(), 0);
    }

    #[test]
    fn test_store_for_each() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        store.put("c.com", &make_meta(3, 1)).unwrap();
        store.put("a.com", &make_meta(1, 1)).unwrap();
        store.put("b.com", &make_meta(2, 1)).unwrap();

        let mut collected = Vec::new();
        store
            .for_each(|key, meta| {
                collected.push((key.to_string(), meta.categories));
                true
            })
            .unwrap();

        // Keys are reversed, so a.com → com.a, b.com → com.b, c.com → com.c.
        assert_eq!(collected.len(), 3);
        assert_eq!(collected[0].0, "com.a");
        assert_eq!(collected[1].0, "com.b");
        assert_eq!(collected[2].0, "com.c");
    }

    #[test]
    fn test_store_keys() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        store.put("a.com", &make_meta(1, 1)).unwrap();
        store.put("b.com", &make_meta(2, 1)).unwrap();

        let keys = store.keys().unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&"com.a".to_string()));
        assert!(keys.contains(&"com.b".to_string()));
    }

    #[test]
    fn test_store_len_and_is_empty() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        assert!(store.is_empty().unwrap());
        store.put("a.com", &make_meta(1, 1)).unwrap();
        assert!(!store.is_empty().unwrap());
        assert_eq!(store.len().unwrap(), 1);
    }

    #[test]
    fn test_store_persistence() {
        let dir = TempDir::new().unwrap();
        let meta = make_meta(1, 1);

        {
            let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();
            store.put("example.com", &meta).unwrap();
        }

        // Reopen and verify data persists.
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();
        assert_eq!(store.get("example.com").unwrap(), Some(meta));
    }

    #[test]
    fn test_store_get_categories() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        // ads (bit 0) | malware (bit 3) = 0b1001
        let bitmap: u32 = (1 << 0) | (1 << 3);
        store
            .put("ads.example.com", &make_meta(bitmap, 1))
            .unwrap();

        assert_eq!(
            store.get_categories("ads.example.com").unwrap(),
            Some(bitmap)
        );
        // Non-existent domain returns None.
        assert_eq!(store.get_categories("nonexistent.com").unwrap(), None);
    }

    #[test]
    fn test_store_get_categories_accumulated() {
        let dir = TempDir::new().unwrap();
        let store = LmdbBlocklistStore::open(dir.path(), None).unwrap();

        // First source: ads (bit 0).
        store.put("example.com", &make_meta(1 << 0, 1)).unwrap();

        // Second source contributes tracker (bit 1) via compile_into-style
        // merge: OR the new category into the existing metadata.
        let existing = store.get("example.com").unwrap().unwrap();
        let merged = BlocklistMetadata {
            categories: existing.categories | (1 << 1),
            sources: existing.sources | 2,
            first_seen: existing.first_seen,
            last_updated: existing.last_updated,
        };
        store.put("example.com", &merged).unwrap();

        // Accumulated bitmap should have both ads and tracker.
        let bitmap = store.get_categories("example.com").unwrap().unwrap();
        assert_eq!(bitmap, (1 << 0) | (1 << 1));
    }
}
