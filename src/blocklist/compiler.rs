//! Blocklist compiler: writes parsed entries to LMDB and builds the Bloom
//! filter.
//!
//! The compiler takes a `Vec<BlocklistEntry>`, opens an LMDB environment,
//! writes all entries with reversed domain keys in a single transaction,
//! and builds a Bloom filter from the reversed keys. Both the LMDB
//! database and the Bloom filter metadata are saved to disk.

use crate::blocklist::bloom::BloomIndex;
use crate::blocklist::storage::LmdbBlocklistStore;
use crate::blocklist::{reverse_domain, BlocklistEntry, BlocklistMetadata, Result};
use std::path::Path;

/// Compiles parsed blocklist entries into LMDB + Bloom filter.
pub struct BlocklistCompiler {
    /// Target false positive rate for the Bloom filter.
    bloom_fpr: f64,
    /// Whether to build a Bloom filter.
    enable_bloom: bool,
}

impl BlocklistCompiler {
    /// Create a new compiler with the given Bloom filter FPR.
    pub fn new(bloom_fpr: f64, enable_bloom: bool) -> Self {
        Self {
            bloom_fpr,
            enable_bloom,
        }
    }

    /// Compile entries into an LMDB store at the given path.
    ///
    /// This writes all entries to LMDB in a single transaction, builds a
    /// Bloom filter from the reversed keys, and optionally saves the Bloom
    /// filter metadata to `bloom_path`.
    ///
    /// Returns the `LmdbBlocklistStore` with the Bloom filter attached.
    pub fn compile<P: AsRef<Path>>(
        &self,
        entries: &[BlocklistEntry],
        lmdb_path: P,
        bloom_path: Option<P>,
    ) -> Result<LmdbBlocklistStore> {
        let now = current_timestamp();

        // Build the Bloom filter from reversed keys (if enabled).
        let reversed_keys: Vec<String> = entries
            .iter()
            .map(|e| reverse_domain(&e.domain))
            .collect();

        let bloom = if self.enable_bloom {
            Some(BloomIndex::build(
                reversed_keys.iter().cloned(),
                entries.len(),
                self.bloom_fpr,
            ))
        } else {
            None
        };

        // Open the LMDB store (with Bloom filter attached for two-stage
        // lookup).
        let store = LmdbBlocklistStore::open(lmdb_path, bloom)?;

        // Clear any existing data.
        store.clear()?;

        // Write all entries in a single batch.
        let batch: Vec<(String, BlocklistMetadata)> = entries
            .iter()
            .map(|e| {
                let meta = BlocklistMetadata {
                    categories: e.categories,
                    sources: e.sources,
                    first_seen: now,
                    last_updated: now,
                };
                (e.domain.clone(), meta)
            })
            .collect();
        store.put_batch_owned(batch)?;

        // Save Bloom filter metadata if requested.
        if let Some(bp) = &bloom_path {
            if let Some(ref bloom) = store.bloom() {
                bloom.save(bp)?;
            }
        }

        Ok(store)
    }

    /// Compile entries into an existing store (for incremental updates).
    ///
    /// Merges new entries with existing ones, updating `last_updated` for
    /// domains that already exist and setting `first_seen` for new ones.
    pub fn compile_into(
        &self,
        entries: &[BlocklistEntry],
        store: &LmdbBlocklistStore,
    ) -> Result<()> {
        let now = current_timestamp();
        let mut batch = Vec::with_capacity(entries.len());

        for entry in entries {
            let existing = store.get(&entry.domain)?;
            let meta = match existing {
                Some(mut prev) => {
                    // Update: merge categories and sources, update timestamp.
                    prev.categories |= entry.categories;
                    prev.sources |= entry.sources;
                    prev.last_updated = now;
                    prev
                }
                None => BlocklistMetadata {
                    categories: entry.categories,
                    sources: entry.sources,
                    first_seen: now,
                    last_updated: now,
                },
            };
            batch.push((entry.domain.clone(), meta));
        }

        let batch_refs: Vec<(&str, &BlocklistMetadata)> =
            batch.iter().map(|(d, m)| (d.as_str(), m)).collect();
        store.put_batch(batch_refs)?;

        Ok(())
    }
}

impl Default for BlocklistCompiler {
    fn default() -> Self {
        Self::new(0.001, true)
    }
}

/// Get the current Unix timestamp (seconds since epoch).
fn current_timestamp() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocklist::categories::{bitmap_has_category, category_to_bit, Category};
    use crate::blocklist::test_utils::TempDir;
    use crate::blocklist::BlocklistStore;

    fn make_entries(n: usize) -> Vec<BlocklistEntry> {
        (0..n)
            .map(|i| BlocklistEntry {
                domain: format!("domain{i}.example.com"),
                categories: 0,
                sources: 1,
            })
            .collect()
    }

    #[test]
    fn test_compile_basic() {
        let dir = TempDir::new().unwrap();
        let entries = make_entries(10);

        let compiler = BlocklistCompiler::default();
        let store = compiler.compile(&entries, dir.path(), None::<&Path>).unwrap();

        // All entries should be found.
        for entry in &entries {
            assert!(
                store.is_blocked(&entry.domain),
                "domain not blocked: {}",
                entry.domain
            );
        }
        assert_eq!(store.len().unwrap(), 10);
    }

    #[test]
    fn test_compile_100_entries() {
        let dir = TempDir::new().unwrap();
        let entries = make_entries(100);

        let compiler = BlocklistCompiler::default();
        let store = compiler.compile(&entries, dir.path(), None::<&Path>).unwrap();

        // Verify lookup works for all entries.
        for entry in &entries {
            let meta = store.lookup(&entry.domain).unwrap();
            assert!(meta.is_some(), "missing: {}", entry.domain);
        }

        // Verify a non-blocked domain is not found.
        assert!(!store.is_blocked("nonexistent.example.com"));
        assert_eq!(store.len().unwrap(), 100);
    }

    #[test]
    fn test_compile_with_bloom_filter() {
        let dir = TempDir::new().unwrap();
        let entries = make_entries(50);

        let compiler = BlocklistCompiler::new(0.001, true);
        let store = compiler.compile(&entries, dir.path(), None::<&Path>).unwrap();

        // Bloom filter should be present.
        assert!(store.bloom().is_some());

        // Two-stage lookup: blocked domains found, non-blocked not found.
        assert!(store.is_blocked("domain0.example.com"));
        assert!(!store.is_blocked("nonexistent.example.com"));
    }

    #[test]
    fn test_compile_without_bloom_filter() {
        let dir = TempDir::new().unwrap();
        let entries = make_entries(10);

        let compiler = BlocklistCompiler::new(0.001, false);
        let store = compiler.compile(&entries, dir.path(), None::<&Path>).unwrap();

        // No Bloom filter.
        assert!(store.bloom().is_none());

        // LMDB lookup still works.
        assert!(store.is_blocked("domain0.example.com"));
        assert!(!store.is_blocked("nonexistent.example.com"));
    }

    #[test]
    fn test_compile_saves_bloom_metadata() {
        let dir = TempDir::new().unwrap();
        let bloom_path = dir.path().join("bloom.meta");
        let entries = make_entries(20);

        let compiler = BlocklistCompiler::default();
        let store = compiler.compile(&entries, dir.path(), Some(&bloom_path)).unwrap();

        // Bloom metadata file should exist.
        assert!(bloom_path.exists());

        // Load the bloom filter from the metadata file + LMDB keys.
        let keys = store.keys().unwrap();
        let loaded_bloom = BloomIndex::load(&bloom_path, keys.into_iter()).unwrap();

        // Loaded bloom should find the same domains.
        assert!(loaded_bloom.contains(&reverse_domain("domain0.example.com")));
        assert!(!loaded_bloom.contains("org.nonexistent.domain"));
    }

    #[test]
    fn test_compile_clears_existing() {
        let dir = TempDir::new().unwrap();
        let entries1 = make_entries(10);
        let entries2: Vec<BlocklistEntry> = (0..5)
            .map(|i| BlocklistEntry {
                domain: format!("new{i}.test.com"),
                categories: 0,
                sources: 2,
            })
            .collect();

        let compiler = BlocklistCompiler::default();

        // First compile.
        {
            let store = compiler.compile(&entries1, dir.path(), None::<&Path>).unwrap();
            assert_eq!(store.len().unwrap(), 10);
        } // store dropped here, releasing the LMDB env

        // Second compile should replace, not append.
        let store = compiler.compile(&entries2, dir.path(), None::<&Path>).unwrap();
        assert_eq!(store.len().unwrap(), 5);
        assert!(store.is_blocked("new0.test.com"));
        assert!(!store.is_blocked("domain0.example.com"));
    }

    #[test]
    fn test_compile_into_merge() {
        let dir = TempDir::new().unwrap();
        let entries1 = make_entries(10);

        let compiler = BlocklistCompiler::default();
        let store = compiler.compile(&entries1, dir.path(), None::<&Path>).unwrap();

        // Compile more entries into the existing store.
        let entries2: Vec<BlocklistEntry> = (0..5)
            .map(|i| BlocklistEntry {
                domain: format!("domain{i}.example.com"), // overlap with entries1
                categories: 2,
                sources: 4,
            })
            .collect();

        compiler.compile_into(&entries2, &store).unwrap();

        // Should still have 10 entries (merged, not duplicated).
        assert_eq!(store.len().unwrap(), 10);

        // Overlapping entries should have merged categories and sources.
        let meta = store.lookup("domain0.example.com").unwrap().unwrap();
        assert_eq!(meta.categories, 2); // merged (0 | 2)
        assert_eq!(meta.sources, 5); // merged (1 | 4)
    }

    #[test]
    fn test_compile_empty() {
        let dir = TempDir::new().unwrap();
        let compiler = BlocklistCompiler::default();
        let store = compiler.compile(&[], dir.path(), None::<&Path>).unwrap();
        assert_eq!(store.len().unwrap(), 0);
        assert!(store.is_empty().unwrap());
    }

    #[test]
    fn test_compile_into_multi_source_category_accumulation() {
        let dir = TempDir::new().unwrap();
        let compiler = BlocklistCompiler::default();

        // Source 1: ads list — tags example.com with the `ads` category.
        let ads_entries = vec![BlocklistEntry {
            domain: "example.com".to_string(),
            categories: category_to_bit(Category::Ads),
            sources: 1,
        }];
        let store = compiler.compile(&ads_entries, dir.path(), None::<&Path>).unwrap();

        // Source 2: malware list — same domain, different category + source.
        let malware_entries = vec![BlocklistEntry {
            domain: "example.com".to_string(),
            categories: category_to_bit(Category::Malware),
            sources: 2,
        }];
        compiler.compile_into(&malware_entries, &store).unwrap();

        // The accumulated bitmap should have both ads and malware bits.
        let meta = store.lookup("example.com").unwrap().unwrap();
        assert!(bitmap_has_category(meta.categories, Category::Ads));
        assert!(bitmap_has_category(meta.categories, Category::Malware));
        assert!(!bitmap_has_category(meta.categories, Category::Tracker));
        assert_eq!(meta.sources, 1 | 2);

        // get_categories should return the same accumulated bitmap.
        let cats = store.get_categories("example.com").unwrap().unwrap();
        assert_eq!(cats, category_to_bit(Category::Ads) | category_to_bit(Category::Malware));
    }

    #[test]
    fn test_compile_populates_categories_from_entries() {
        let dir = TempDir::new().unwrap();
        let compiler = BlocklistCompiler::default();

        let entries = vec![
            BlocklistEntry {
                domain: "ads.example.com".to_string(),
                categories: category_to_bit(Category::Ads),
                sources: 1,
            },
            BlocklistEntry {
                domain: "tracker.example.com".to_string(),
                categories: category_to_bit(Category::Tracker) | category_to_bit(Category::Telemetry),
                sources: 1,
            },
        ];
        let store = compiler.compile(&entries, dir.path(), None::<&Path>).unwrap();

        let ads_meta = store.get_categories("ads.example.com").unwrap().unwrap();
        assert_eq!(ads_meta, category_to_bit(Category::Ads));

        let tracker_meta = store.get_categories("tracker.example.com").unwrap().unwrap();
        assert_eq!(
            tracker_meta,
            category_to_bit(Category::Tracker) | category_to_bit(Category::Telemetry)
        );
    }
}
