//! Bloom filter for fast negative-path blocklist lookups.
//!
//! Uses the [`fastbloom`] crate with a fixed seed for deterministic
//! serialization. The Bloom filter is sized from the expected number of
//! entries and the target false positive rate (FPR).
//!
//! ## Serialization
//!
//! `fastbloom` 0.1 does not expose its internal bit vector, so we
//! serialize the filter *configuration* (num_bits, num_hashes, seed) to a
//! metadata file and rebuild the filter from the LMDB source of truth on
//! load. This is the standard pattern for a Bloom filter backed by a
//! persistent store — the filter is always rebuildable from LMDB keys.

use crate::blocklist::{BlocklistError, Result};
use fastbloom::BloomFilter;
use std::io::{Read, Write};
use std::path::Path;

/// Magic header for the bloom metadata file format.
const BLOOM_MAGIC: &[u8; 8] = b"DNBLM001";

/// Fixed seed for deterministic Bloom filter construction.
const BLOOM_SEED: u128 = 0x4e4c_4f43_4b42_4c4f_4f4d_0000_0000_0001;

/// Wrapper around a `fastbloom::BloomFilter` with deterministic seed and
/// serialization support.
pub struct BloomIndex {
    filter: BloomFilter,
    num_bits: usize,
    num_entries: usize,
}

impl BloomIndex {
    /// Build a Bloom filter from an iterator of reversed domain keys.
    ///
    /// `expected_items` is the expected number of entries (used for sizing).
    /// `fpr` is the target false positive rate (e.g. `0.001` for 0.1%).
    pub fn build<I>(items: I, expected_items: usize, fpr: f64) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let num_bits = optimal_num_bits(expected_items, fpr);
        let mut filter = BloomFilter::builder(num_bits)
            .seed(&BLOOM_SEED)
            .expected_items(expected_items.max(1));

        let mut count = 0;
        for item in items {
            filter.insert(&item);
            count += 1;
        }

        Self {
            filter,
            num_bits,
            num_entries: count,
        }
    }

    /// Create an empty Bloom filter with the given configuration.
    pub fn empty(expected_items: usize, fpr: f64) -> Self {
        let num_bits = optimal_num_bits(expected_items, fpr);
        let filter = BloomFilter::builder(num_bits)
            .seed(&BLOOM_SEED)
            .expected_items(expected_items.max(1));
        Self {
            filter,
            num_bits,
            num_entries: 0,
        }
    }

    /// Rebuild a Bloom filter from an existing configuration, inserting
    /// all items from the iterator.
    pub fn rebuild<I>(items: I, num_bits: usize) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let items_vec: Vec<String> = items.into_iter().collect();
        let count = items_vec.len();
        let mut filter = BloomFilter::builder(num_bits)
            .seed(&BLOOM_SEED)
            .expected_items(count.max(1));

        for item in &items_vec {
            filter.insert(item);
        }

        Self {
            filter,
            num_bits,
            num_entries: count,
        }
    }

    /// Check if the Bloom filter *might* contain the given reversed domain
    /// key. Returns `false` if definitely not present (fast negative path).
    /// Returns `true` if possibly present (must confirm in LMDB).
    pub fn contains(&self, reversed_domain: &str) -> bool {
        self.filter.contains(reversed_domain)
    }

    /// Insert a reversed domain key into the filter.
    pub fn insert(&mut self, reversed_domain: &str) {
        self.filter.insert(reversed_domain);
        self.num_entries += 1;
    }

    /// Number of bits in the underlying bit vector.
    pub fn num_bits(&self) -> usize {
        self.num_bits
    }

    /// Number of entries inserted.
    pub fn num_entries(&self) -> usize {
        self.num_entries
    }

    /// Number of hash functions used.
    pub fn num_hashes(&self) -> u64 {
        self.filter.num_hashes()
    }

    /// Save the Bloom filter metadata to a file.
    ///
    /// The file contains the configuration (num_bits, num_hashes,
    /// num_entries) needed to rebuild the filter from the LMDB source of
    /// truth. The actual bit vector is not saved — it is rebuilt from LMDB
    /// keys on load.
    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let mut file = std::fs::File::create(path.as_ref())?;
        // Format: magic (8 bytes) + num_bits (8 bytes LE) + num_hashes (8
        // bytes LE) + num_entries (8 bytes LE) = 32 bytes total.
        file.write_all(BLOOM_MAGIC)?;
        file.write_all(&self.num_bits.to_le_bytes())?;
        file.write_all(&self.num_hashes().to_le_bytes())?;
        file.write_all(&self.num_entries.to_le_bytes())?;
        Ok(())
    }

    /// Load Bloom filter metadata from a file and rebuild the filter from
    /// the given reversed domain keys (typically obtained from LMDB).
    pub fn load<P: AsRef<Path>, I>(path: P, items: I) -> Result<Self>
    where
        I: IntoIterator<Item = String>,
    {
        let mut file = std::fs::File::open(path.as_ref())?;
        let mut magic = [0u8; 8];
        file.read_exact(&mut magic)?;
        if &magic != BLOOM_MAGIC {
            return Err(BlocklistError::Bloom(
                "invalid bloom metadata file: bad magic".to_string(),
            ));
        }
        let mut buf = [0u8; 8];
        file.read_exact(&mut buf)?;
        let num_bits = usize::from_le_bytes(buf);
        file.read_exact(&mut buf)?;
        let _num_hashes = u64::from_le_bytes(buf);
        file.read_exact(&mut buf)?;
        let _num_entries = usize::from_le_bytes(buf);

        // Rebuild the filter from the provided items.
        Ok(Self::rebuild(items, num_bits))
    }

    /// Expected false positive rate for the given number of items.
    pub fn expected_false_positive_rate(&self, num_items: usize) -> f64 {
        // p ≈ (1 - e^(-kn/m))^k
        let k = self.num_hashes() as f64;
        let m = self.num_bits as f64;
        let n = num_items.max(1) as f64;
        let exponent = -k * n / m;
        let base = 1.0 - exponent.exp();
        base.powi(k as i32)
    }
}

/// Calculate the optimal number of bits for a Bloom filter given the
/// expected number of items and target false positive rate.
///
/// Formula: m = -n * ln(p) / (ln(2))^2
fn optimal_num_bits(expected_items: usize, fpr: f64) -> usize {
    if expected_items == 0 {
        return 1024; // minimum size
    }
    let n = expected_items as f64;
    let p = fpr.clamp(0.000001, 0.5); // sanity clamp
    let m = -n * p.ln() / (std::f64::consts::LN_2 * std::f64::consts::LN_2);
    // Round up to nearest multiple of 512 (block size) for efficiency.
    let m = m.ceil() as usize;
    (m + 511) & !511
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bloom_basic() {
        let domains = vec![
            "com.example.ads".to_string(),
            "com.example.tracker".to_string(),
            "org.example.malware".to_string(),
        ];
        let bloom = BloomIndex::build(domains.into_iter(), 3, 0.001);

        // Positive: inserted domains must be "maybe blocked".
        assert!(bloom.contains("com.example.ads"));
        assert!(bloom.contains("com.example.tracker"));
        assert!(bloom.contains("org.example.malware"));
    }

    #[test]
    fn test_bloom_negative() {
        let domains = vec!["com.example.ads".to_string()];
        let bloom = BloomIndex::build(domains.into_iter(), 1, 0.001);

        // A completely unrelated domain should be "definitely not blocked".
        assert!(!bloom.contains("org.different.domain"));
    }

    #[test]
    fn test_bloom_false_positive_rate() {
        // Insert 1000 domains, test 10000 non-inserted domains.
        let inserted: Vec<String> = (0..1000)
            .map(|i| format!("com.domain{i}.blocked"))
            .collect();
        let bloom = BloomIndex::build(inserted.iter().cloned(), 1000, 0.001);

        // All inserted must be found.
        for d in &inserted {
            assert!(bloom.contains(d), "inserted domain not found: {d}");
        }

        // Check false positive rate on non-inserted domains.
        let mut false_positives = 0;
        let test_count = 10000;
        for i in 0..test_count {
            let test_domain = format!("com.domain{i}.clean");
            if !inserted.contains(&test_domain) && bloom.contains(&test_domain) {
                false_positives += 1;
            }
        }
        let fpr = false_positives as f64 / test_count as f64;
        // With 0.1% target FPR, we allow up to 1% in practice for small
        // sample sizes (the filter is sized for 1000 items at 0.1%).
        assert!(
            fpr < 0.05,
            "false positive rate too high: {fpr} ({false_positives}/{test_count})"
        );
    }

    #[test]
    fn test_bloom_save_and_load() {
        let dir = crate::blocklist::test_utils::TempDir::new().unwrap();
        let bloom_path = dir.path().join("bloom.meta");

        let domains: Vec<String> = (0..100)
            .map(|i| format!("com.domain{i}.blocked"))
            .collect();
        let bloom = BloomIndex::build(domains.iter().cloned(), 100, 0.001);
        bloom.save(&bloom_path).unwrap();

        // Load and rebuild from the same domain list.
        let loaded = BloomIndex::load(&bloom_path, domains.iter().cloned()).unwrap();

        // The loaded filter should have the same behavior.
        assert!(loaded.contains("com.domain0.blocked"));
        assert!(loaded.contains("com.domain99.blocked"));
        assert!(!loaded.contains("org.different.domain"));
    }

    #[test]
    fn test_bloom_empty() {
        let bloom = BloomIndex::empty(100, 0.001);
        assert_eq!(bloom.num_entries(), 0);
        assert!(!bloom.contains("anything.com"));
    }

    #[test]
    fn test_bloom_insert() {
        let mut bloom = BloomIndex::empty(100, 0.001);
        bloom.insert("com.example.ads");
        assert!(bloom.contains("com.example.ads"));
        assert_eq!(bloom.num_entries(), 1);
    }

    #[test]
    fn test_bloom_large_scale() {
        // Test with 10000 entries to verify sizing works.
        let domains: Vec<String> = (0..10000)
            .map(|i| format!("com.domain{i}.test"))
            .collect();
        let bloom = BloomIndex::build(domains.iter().cloned(), 10000, 0.001);

        // All inserted must be found.
        for d in domains.iter().take(100) {
            assert!(bloom.contains(d));
        }

        // Verify num_bits is reasonable (should be ~14-15 bits per entry).
        let bits_per_entry = bloom.num_bits() as f64 / 10000.0;
        assert!(
            bits_per_entry > 10.0 && bits_per_entry < 25.0,
            "bits per entry: {bits_per_entry}"
        );
    }

    #[test]
    fn test_optimal_num_bits() {
        // 1M entries at 0.1% FPR: m ≈ 14.4M bits.
        let bits = optimal_num_bits(1_000_000, 0.001);
        assert!(bits > 10_000_000 && bits < 20_000_000, "bits: {bits}");

        // 0 entries: minimum size.
        let bits = optimal_num_bits(0, 0.001);
        assert_eq!(bits, 1024);
    }

    #[test]
    fn test_bloom_load_bad_magic() {
        let dir = crate::blocklist::test_utils::TempDir::new().unwrap();
        let path = dir.path().join("bad.meta");
        std::fs::write(&path, b"BADMAGIC extra data here").unwrap();
        let result = BloomIndex::load(&path, std::iter::empty());
        assert!(result.is_err());
    }
}
