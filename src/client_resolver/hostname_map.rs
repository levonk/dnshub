//! Hostname → profile mapping for DHCP dynamic lease resolution
//! (PRD section 4.5, line 1052).
//!
//! When a client is identified by a DHCP dynamic lease, the lease carries a
//! hostname (DHCP option 12) but not an explicit profile. The
//! `[dhcp_integration].hostname_map` table in `policy.toml` maps hostnames to
//! profile names:
//!
//! ```toml
//! [dhcp_integration]
//! hostname_map = { "kids-tablet" = "kids", "dad-laptop" = "parents" }
//! ```
//!
//! [`HostnameMap`] is an in-memory cache of this table, built from
//! [`crate::policy::config::PolicyConfig`] and consulted by
//! [`crate::client_resolver::ClientResolver`] after a dynamic lease lookup
//! returns a hostname.

use std::collections::HashMap;
use std::sync::Arc;

/// In-memory cache of hostname → profile name mappings.
///
/// Built from the `[dhcp_integration].hostname_map` section of `policy.toml`.
/// Lookup is case-insensitive (DHCP hostnames are conventionally lowercase,
/// but clients may send any casing in option 12).
///
/// `HostnameMap` is cheap to clone (inner state is behind an `Arc`).
#[derive(Clone, Default)]
pub struct HostnameMap {
    inner: Arc<HashMap<String, String>>,
}

impl HostnameMap {
    /// Build a `HostnameMap` from a raw `hostname → profile` map.
    ///
    /// Keys are normalised to lowercase so that [`Self::resolve`] is
    /// case-insensitive.
    pub fn new(map: HashMap<String, String>) -> Self {
        let normalised: HashMap<String, String> = map
            .into_iter()
            .map(|(k, v)| (k.to_ascii_lowercase(), v))
            .collect();
        Self {
            inner: Arc::new(normalised),
        }
    }

    /// Build an empty `HostnameMap` (no hostname mappings).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Resolve a hostname to a profile name.
    ///
    /// Returns `None` if the hostname is not in the map. The lookup is
    /// case-insensitive: `"Kids-Tablet"` matches a `"kids-tablet"` key.
    pub fn resolve(&self, hostname: &str) -> Option<&str> {
        let key = hostname.to_ascii_lowercase();
        self.inner.get(&key).map(|s| s.as_str())
    }

    /// Returns `true` if the map contains no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of hostname → profile entries.
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

impl PartialEq for HostnameMap {
    fn eq(&self, other: &Self) -> bool {
        *self.inner == *other.inner
    }
}

impl Eq for HostnameMap {}

impl std::fmt::Debug for HostnameMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.inner.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map_of(pairs: &[(&str, &str)]) -> HostnameMap {
        let mut h = HashMap::new();
        for (k, v) in pairs {
            h.insert((*k).to_string(), (*v).to_string());
        }
        HostnameMap::new(h)
    }

    #[test]
    fn test_resolve_match() {
        let map = map_of(&[("kids-tablet", "kids"), ("dad-laptop", "parents")]);
        assert_eq!(map.resolve("kids-tablet"), Some("kids"));
        assert_eq!(map.resolve("dad-laptop"), Some("parents"));
    }

    #[test]
    fn test_resolve_no_match() {
        let map = map_of(&[("kids-tablet", "kids")]);
        assert_eq!(map.resolve("unknown-host"), None);
    }

    #[test]
    fn test_empty_map() {
        let map = HostnameMap::empty();
        assert!(map.is_empty());
        assert_eq!(map.len(), 0);
        assert_eq!(map.resolve("anything"), None);
    }

    #[test]
    fn test_case_insensitive_lookup() {
        let map = map_of(&[("kids-tablet", "kids")]);
        // Hostname sent with different casing still matches.
        assert_eq!(map.resolve("Kids-Tablet"), Some("kids"));
        assert_eq!(map.resolve("KIDS-TABLET"), Some("kids"));
        assert_eq!(map.resolve("kids-tablet"), Some("kids"));
    }

    #[test]
    fn test_keys_normalised_to_lowercase() {
        // Keys with uppercase are normalised on insert.
        let mut h = HashMap::new();
        h.insert("Dad-Laptop".to_string(), "parents".to_string());
        let map = HostnameMap::new(h);
        assert_eq!(map.resolve("dad-laptop"), Some("parents"));
        assert_eq!(map.resolve("DAD-LAPTOP"), Some("parents"));
    }

    #[test]
    fn test_len_and_is_empty() {
        let map = map_of(&[("a", "x"), ("b", "y")]);
        assert!(!map.is_empty());
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn test_clone_is_independent_view() {
        let map = map_of(&[("kids-tablet", "kids")]);
        let cloned = map.clone();
        assert_eq!(cloned.resolve("kids-tablet"), Some("kids"));
        // Both share the same underlying data (Arc).
        assert_eq!(map, cloned);
    }
}
