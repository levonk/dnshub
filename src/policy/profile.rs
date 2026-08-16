//! Per-client policy profile definition (PRD section 4.5, lines 975-1052).
//!
//! A [`PolicyProfile`] describes the DNS filtering policy applied to a single
//! client or group of clients: which blocklist categories to block, which to
//! allow, explicit custom allow/block lists, the upstream tier to use, and a
//! log level for policy decisions.

use serde::{Deserialize, Serialize};

/// A per-client policy profile.
///
/// Mirrors the `[policies.X]` / `[default]` tables in `policy.toml`
/// (PRD lines 975-1052). Each field maps 1:1 to a TOML key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyProfile {
    /// Profile name, e.g. `"kids"`, `"parents"`, `"default"`.
    #[serde(default)]
    pub name: String,

    /// Human-readable description shown in the UI.
    #[serde(default)]
    pub description: String,

    /// Blocklist categories to block for this profile
    /// (e.g. `["ads", "tracker", "malware"]`).
    #[serde(default)]
    pub blocked_categories: Vec<String>,

    /// Categories explicitly allowed (overrides `blocked_categories`).
    /// The special value `"all"` disables category-based blocking entirely.
    #[serde(default)]
    pub allowed_categories: Vec<String>,

    /// Domains always allowed, bypassing all blocklist checks.
    /// Supports glob patterns (`*.example.com`).
    #[serde(default)]
    pub custom_allowlist: Vec<String>,

    /// Domains always blocked, regardless of blocklist state.
    /// Supports glob patterns (`*.example.com`).
    #[serde(default)]
    pub custom_blocklist: Vec<String>,

    /// Upstream tier reference, e.g. `"tiered"`.
    #[serde(default = "default_upstream")]
    pub upstream: String,

    /// Log level for policy decisions for this profile
    /// (`"error"`, `"warn"`, `"info"`, `"debug"`).
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_upstream() -> String {
    "tiered".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

impl PolicyProfile {
    /// Returns `true` if `allowed_categories` contains the special `"all"`
    /// entry, which disables category-based blocking for this profile.
    pub fn allows_all_categories(&self) -> bool {
        self.allowed_categories
            .iter()
            .any(|c| c.eq_ignore_ascii_case("all"))
    }

    /// Returns `true` if this profile blocks the given category name.
    ///
    /// A category is blocked if it appears in `blocked_categories` **and**
    /// is not overridden by `allowed_categories` (or `"all"`).
    pub fn blocks_category(&self, category: &str) -> bool {
        if self.allows_all_categories() {
            return false;
        }
        if self.allowed_categories.iter().any(|c| c == category) {
            return false;
        }
        self.blocked_categories.iter().any(|c| c == category)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allows_all_categories() {
        let p = PolicyProfile {
            allowed_categories: vec!["all".to_string()],
            blocked_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        assert!(p.allows_all_categories());
        assert!(!p.blocks_category("ads"));
    }

    #[test]
    fn test_blocks_category() {
        let p = PolicyProfile {
            blocked_categories: vec!["ads".to_string(), "tracker".to_string()],
            ..Default::default()
        };
        assert!(p.blocks_category("ads"));
        assert!(p.blocks_category("tracker"));
        assert!(!p.blocks_category("malware"));
    }

    #[test]
    fn test_allowed_category_overrides_blocked() {
        let p = PolicyProfile {
            blocked_categories: vec!["ads".to_string()],
            allowed_categories: vec!["ads".to_string()],
            ..Default::default()
        };
        assert!(!p.blocks_category("ads"));
    }

    #[test]
    fn test_serde_roundtrip() {
        let p = PolicyProfile {
            name: "kids".to_string(),
            description: "kids profile".to_string(),
            blocked_categories: vec!["ads".to_string()],
            allowed_categories: vec![],
            custom_allowlist: vec!["khanacademy.org".to_string()],
            custom_blocklist: vec!["tiktok.com".to_string()],
            upstream: "tiered".to_string(),
            log_level: "warn".to_string(),
        };
        let s = toml::to_string(&p).unwrap();
        let p2: PolicyProfile = toml::from_str(&s).unwrap();
        assert_eq!(p, p2);
    }
}
