//! Serde structs for `blocklists.toml`.
//!
//! Configuration shape:
//!
//! ```toml
//! [[sources]]
//! name = "hagezi-normal"
//! url = "https://example.com/hagezi.txt"
//! format = "domains"
//! categories = ["ads", "tracker"]
//! refresh_hours = 24
//!
//! [storage]
//! type = "lmdb"
//! path = "/var/lib/dnshub/blocklist.mdb"
//! bloom_filter = true
//! bloom_fpr = 0.001
//! ```

use serde::{Deserialize, Serialize};

use crate::blocklist::categories::{bitmap_from_names, CategoryBitmap};

/// Blocklist source format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    /// Hosts file format: `0.0.0.0 domain` or `127.0.0.1 domain`.
    Hosts,
    /// Domains list format: one domain per line, `#` comments.
    Domains,
    /// Adblock Plus format (parser added in story 02-002).
    Adblock,
}

impl Default for Format {
    fn default() -> Self {
        Self::Domains
    }
}

/// A single blocklist source configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Human-readable name for this source (e.g. `hagezi-normal`).
    pub name: String,
    /// URL to fetch the blocklist from.
    pub url: String,
    /// File format of the source.
    #[serde(default)]
    pub format: Format,
    /// Category labels for this source (e.g. `["ads", "tracker"]`).
    ///
    /// These are converted to a [`CategoryBitmap`] via
    /// [`SourceConfig::category_bitmap`]. Unknown names are silently
    /// ignored.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Refresh interval in hours. Mutually exclusive with `refresh_minutes`.
    #[serde(default)]
    pub refresh_hours: Option<u32>,
    /// Refresh interval in minutes. Mutually exclusive with `refresh_hours`.
    #[serde(default)]
    pub refresh_minutes: Option<u32>,
}

impl SourceConfig {
    /// Returns the refresh interval in seconds.
    ///
    /// Defaults to 24 hours if neither `refresh_hours` nor `refresh_minutes`
    /// is specified. If both are set, `refresh_minutes` takes precedence.
    pub fn refresh_interval_secs(&self) -> u64 {
        if let Some(mins) = self.refresh_minutes {
            return u64::from(mins) * 60;
        }
        if let Some(hours) = self.refresh_hours {
            return u64::from(hours) * 3600;
        }
        24 * 3600 // default: daily
    }

    /// Returns the category bitmap for this source, computed from the
    /// `categories` name list.
    ///
    /// Each name is mapped to a [`Category`][crate::blocklist::Category] bit
    /// and OR'd together. Unknown names are silently ignored (they contribute
    /// no bits). Returns `0` when no categories are configured.
    pub fn category_bitmap(&self) -> CategoryBitmap {
        bitmap_from_names(self.categories.iter().map(String::as_str))
    }
}

/// Storage backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Storage type. Currently only `"lmdb"` is supported.
    #[serde(default = "default_storage_type")]
    #[serde(rename = "type")]
    pub storage_type: String,
    /// Filesystem path for the LMDB environment directory.
    pub path: String,
    /// Whether to enable the Bloom filter front-end.
    #[serde(default = "default_bloom_filter")]
    pub bloom_filter: bool,
    /// Target false positive rate for the Bloom filter (e.g. `0.001` = 0.1%).
    #[serde(default = "default_bloom_fpr")]
    pub bloom_fpr: f64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            storage_type: default_storage_type(),
            path: String::from("blocklist.mdb"),
            bloom_filter: true,
            bloom_fpr: default_bloom_fpr(),
        }
    }
}

fn default_storage_type() -> String {
    String::from("lmdb")
}

fn default_bloom_filter() -> bool {
    true
}

fn default_bloom_fpr() -> f64 {
    0.001 // 0.1%
}

/// Top-level blocklists configuration (the full `blocklists.toml`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BlocklistsConfig {
    /// Blocklist sources to fetch.
    #[serde(default)]
    pub sources: Vec<SourceConfig>,
    /// Storage configuration.
    #[serde(default)]
    pub storage: StorageConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let toml_str = r#"
[[sources]]
name = "hagezi-normal"
url = "https://example.com/hagezi.txt"
format = "domains"
categories = ["ads", "tracker"]
refresh_hours = 24

[[sources]]
name = "urlhaus"
url = "https://example.com/urlhaus.csv"
format = "domains"
categories = ["malware"]
refresh_minutes = 15

[storage]
type = "lmdb"
path = "/var/lib/dnshub/blocklist.mdb"
bloom_filter = true
bloom_fpr = 0.001
"#;
        let config: BlocklistsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.sources[0].name, "hagezi-normal");
        assert_eq!(config.sources[0].format, Format::Domains);
        assert_eq!(config.sources[0].refresh_interval_secs(), 86400);
        assert_eq!(config.sources[1].name, "urlhaus");
        assert_eq!(config.sources[1].refresh_interval_secs(), 900);
        assert_eq!(config.storage.storage_type, "lmdb");
        assert!(config.storage.bloom_filter);
        assert!((config.storage.bloom_fpr - 0.001).abs() < f64::EPSILON);
    }

    #[test]
    fn test_default_refresh_interval() {
        let source = SourceConfig {
            name: "test".to_string(),
            url: "https://example.com/list.txt".to_string(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        };
        assert_eq!(source.refresh_interval_secs(), 86400);
    }

    #[test]
    fn test_empty_config() {
        let toml_str = "";
        let config: BlocklistsConfig = toml::from_str(toml_str).unwrap();
        assert!(config.sources.is_empty());
        assert!(config.storage.bloom_filter);
    }

    #[test]
    fn test_hosts_format() {
        let toml_str = r#"
[[sources]]
name = "stevenblack"
url = "https://example.com/hosts"
format = "hosts"
"#;
        let config: BlocklistsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.sources[0].format, Format::Hosts);
    }

    #[test]
    fn test_source_category_bitmap() {
        let source = SourceConfig {
            name: "test".to_string(),
            url: "https://example.com/list.txt".to_string(),
            format: Format::Domains,
            categories: vec!["ads".to_string(), "tracker".to_string()],
            refresh_hours: None,
            refresh_minutes: None,
        };
        assert_eq!(source.category_bitmap(), (1 << 0) | (1 << 1));
    }

    #[test]
    fn test_source_category_bitmap_ignores_unknown() {
        let source = SourceConfig {
            name: "test".to_string(),
            url: "https://example.com/list.txt".to_string(),
            format: Format::Domains,
            categories: vec!["ads".to_string(), "bogus".to_string(), "malware".to_string()],
            refresh_hours: None,
            refresh_minutes: None,
        };
        assert_eq!(source.category_bitmap(), (1 << 0) | (1 << 3));
    }

    #[test]
    fn test_source_category_bitmap_empty() {
        let source = SourceConfig {
            name: "test".to_string(),
            url: "https://example.com/list.txt".to_string(),
            format: Format::Domains,
            categories: vec![],
            refresh_hours: None,
            refresh_minutes: None,
        };
        assert_eq!(source.category_bitmap(), 0);
    }
}
