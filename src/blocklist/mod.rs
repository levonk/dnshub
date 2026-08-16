//! Blocklist storage (LMDB + Bloom filter) and daemon.
//!
//! This module implements the blocklist storage system described in PRD
//! section 4.2. It uses LMDB (via the [`heed`] crate) with reversed domain
//! keys for prefix search, fronted by a two-stage Bloom filter for fast
//! negative lookups. A background daemon fetches blocklist sources over
//! HTTP, parses them, and compiles entries into LMDB with atomic hot-swap
//! via [`ArcSwap`].
//!
//! ## Architecture
//!
//! ```text
//!  DNS query
//!    │
//!    ▼
//!  Bloom filter  ── "definitely not blocked" ──▶  return immediately
//!    │ "maybe blocked"
//!    ▼
//!  LMDB lookup   ──▶  BlocklistMetadata (categories, sources, timestamps)
//! ```
//!
//! ## Key layout
//!
//! Keys are reversed domain names (`net.doubleclick.www` for
//! `www.doubleclick.net`) so that all subdomains of a blocked domain sort
//! contiguously, enabling prefix search via LMDB cursor (`MDB_SET_RANGE`).

pub mod bloom;
pub mod boot;
pub mod categories;
pub mod circuit_breaker;
pub mod backoff;
pub mod compiler;
pub mod config;
pub mod daemon;
pub mod health;
pub mod hot_swap;
pub mod parser;
pub mod storage;

#[cfg(test)]
mod test_utils;

pub use bloom::BloomIndex;
pub use categories::{
    bitmap_from_categories, bitmap_from_names, bitmap_has_category, bitmap_to_categories,
    bitmap_to_names, category_from_str, category_to_bit, Category, CategoryBitmap,
};
pub use compiler::BlocklistCompiler;
pub use config::{BlocklistsConfig, FailureHandlingConfig, Format, SourceConfig, StorageConfig};
pub use daemon::BlocklistDaemon;
pub use health::{SourceHealth, SourceHealthRegistry};
pub use hot_swap::HotSwapStore;
pub use parser::{parse_adblock, parse_domains, parse_hosts, parse_source};
pub use storage::LmdbBlocklistStore;

use std::fmt;

/// A single blocklist entry parsed from a source file.
///
/// The `domain` field is the forward domain (e.g. `ads.example.com`).
/// The compiler reverses it before writing to LMDB.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlocklistEntry {
    /// Forward domain name (e.g. `ads.example.com`).
    pub domain: String,
    /// Category bitmap (u32). In this story all entries get `category = 0`;
    /// category population arrives in story 02-002.
    pub categories: u32,
    /// Source list ID (bitmap of which source list(s) contributed this entry).
    pub sources: u16,
}

/// Per-domain metadata stored as the LMDB value.
///
/// Serialized as 14 bytes (native endian):
/// - `categories`: 4 bytes (u32)
/// - `sources`: 2 bytes (u16)
/// - `first_seen`: 4 bytes (u32, unix timestamp)
/// - `last_updated`: 4 bytes (u32, unix timestamp)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlocklistMetadata {
    /// Category bitmap (u32 bitmask: ads|tracker|malware|…).
    pub categories: u32,
    /// Bitmap of source list IDs that contributed this domain.
    pub sources: u16,
    /// Unix timestamp when this domain was first seen.
    pub first_seen: u32,
    /// Unix timestamp when this domain was last updated.
    pub last_updated: u32,
}

impl BlocklistMetadata {
    /// Number of bytes in the serialized representation.
    pub const SERIALIZED_SIZE: usize = 14;

    /// Serialize to a 14-byte array (native endian).
    pub fn to_bytes(&self) -> [u8; Self::SERIALIZED_SIZE] {
        let mut buf = [0u8; Self::SERIALIZED_SIZE];
        buf[0..4].copy_from_slice(&self.categories.to_ne_bytes());
        buf[4..6].copy_from_slice(&self.sources.to_ne_bytes());
        buf[6..10].copy_from_slice(&self.first_seen.to_ne_bytes());
        buf[10..14].copy_from_slice(&self.last_updated.to_ne_bytes());
        buf
    }

    /// Deserialize from a byte slice (native endian).
    ///
    /// Returns an error if the slice is not exactly 14 bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != Self::SERIALIZED_SIZE {
            return Err(BlocklistError::InvalidMetadataSize {
                expected: Self::SERIALIZED_SIZE,
                actual: bytes.len(),
            });
        }
        let categories = u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let sources = u16::from_ne_bytes([bytes[4], bytes[5]]);
        let first_seen = u32::from_ne_bytes([bytes[6], bytes[7], bytes[8], bytes[9]]);
        let last_updated = u32::from_ne_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]);
        Ok(Self {
            categories,
            sources,
            first_seen,
            last_updated,
        })
    }
}

/// Trait for blocklist storage backends.
///
/// The primary implementation is [`LmdbBlocklistStore`], which combines a
/// Bloom filter (fast negative path) with LMDB (positive confirmation +
/// metadata retrieval).
pub trait BlocklistStore: Send + Sync {
    /// Look up a domain and return its metadata if blocked.
    ///
    /// This performs the two-stage lookup: Bloom filter first (fast negative),
    /// then LMDB confirmation if the Bloom filter says "maybe".
    fn lookup(&self, domain: &str) -> Result<Option<BlocklistMetadata>>;

    /// Returns `true` if the domain is blocked.
    fn is_blocked(&self, domain: &str) -> bool {
        self.lookup(domain).map(|m| m.is_some()).unwrap_or(false)
    }

    /// Number of entries in the store.
    fn len(&self) -> Result<u64>;

    /// Returns `true` if the store contains no entries.
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
}

/// Reverse a domain name for LMDB key ordering.
///
/// `www.example.com` → `com.example.www`
///
/// This enables prefix search: all subdomains of `example.com` sort
/// contiguously under the prefix `com.example.`.
pub fn reverse_domain(domain: &str) -> String {
    let trimmed = domain.trim().trim_end_matches('.');
    if trimmed.is_empty() {
        return String::new();
    }
    trimmed
        .split('.')
        .rev()
        .collect::<Vec<_>>()
        .join(".")
}

/// Errors returned by blocklist operations.
#[derive(Debug)]
pub enum BlocklistError {
    /// LMDB / heed error.
    Heed(heed::Error),
    /// I/O error (file system).
    Io(std::io::Error),
    /// HTTP fetch error.
    Http(String),
    /// Invalid metadata byte length.
    InvalidMetadataSize { expected: usize, actual: usize },
    /// Schema validation failure (HTTP status, content-type, size, entry count).
    SchemaValidation(String),
    /// Parse error for a blocklist source file.
    Parse(String),
    /// Bloom filter error (sizing, serialization).
    Bloom(String),
    /// LMDB environment path does not exist or cannot be opened.
    EnvOpen(String),
}

impl fmt::Display for BlocklistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heed(e) => write!(f, "LMDB error: {e}"),
            Self::Io(e) => write!(f, "I/O error: {e}"),
            Self::Http(msg) => write!(f, "HTTP error: {msg}"),
            Self::InvalidMetadataSize { expected, actual } => {
                write!(f, "invalid metadata size: expected {expected}, got {actual}")
            }
            Self::SchemaValidation(msg) => write!(f, "schema validation failed: {msg}"),
            Self::Parse(msg) => write!(f, "parse error: {msg}"),
            Self::Bloom(msg) => write!(f, "bloom filter error: {msg}"),
            Self::EnvOpen(msg) => write!(f, "failed to open LMDB environment: {msg}"),
        }
    }
}

impl std::error::Error for BlocklistError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Heed(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<heed::Error> for BlocklistError {
    fn from(e: heed::Error) -> Self {
        Self::Heed(e)
    }
}

impl From<std::io::Error> for BlocklistError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// Convenience `Result` alias for blocklist operations.
pub type Result<T> = std::result::Result<T, BlocklistError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reverse_domain() {
        assert_eq!(reverse_domain("www.example.com"), "com.example.www");
        assert_eq!(reverse_domain("example.com"), "com.example");
        assert_eq!(reverse_domain("a.b.c.d.e"), "e.d.c.b.a");
        assert_eq!(reverse_domain("single"), "single");
        assert_eq!(reverse_domain(""), "");
        assert_eq!(reverse_domain("example.com."), "com.example");
        assert_eq!(reverse_domain("  example.com  "), "com.example");
    }

    #[test]
    fn test_metadata_roundtrip() {
        let meta = BlocklistMetadata {
            categories: 0x0000_000F,
            sources: 0x0003,
            first_seen: 1_700_000_000,
            last_updated: 1_700_010_000,
        };
        let bytes = meta.to_bytes();
        assert_eq!(bytes.len(), BlocklistMetadata::SERIALIZED_SIZE);
        let decoded = BlocklistMetadata::from_bytes(&bytes).unwrap();
        assert_eq!(meta, decoded);
    }

    #[test]
    fn test_metadata_invalid_size() {
        let result = BlocklistMetadata::from_bytes(&[0u8; 10]);
        assert!(result.is_err());
    }
}
