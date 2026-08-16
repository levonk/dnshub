//! Client IP → policy profile resolution (PRD section 4.5, lines 963-968).
//!
//! [`ClientResolver`] maps a client IP address to a policy profile name. The
//! resolution order is:
//!
//! 1. **Static IP exact match** — `[clients."192.168.1.10"]` in `policy.toml`.
//! 2. **CIDR range match** — `[clients."192.168.10.0/24"]` in `policy.toml`.
//! 3. **Default profile** — the `[default]` profile name.
//!
//! DHCP lease lookup (steps that would come before static IP) is added in
//! story 04-011. In this story the resolver uses only static IP + CIDR +
//! default fallback.

pub mod cidr;
pub mod config;

pub use cidr::{find_matching_cidr, match_ip, CidrEntry};

use crate::client_resolver::config::split_mappings;
use crate::policy::config::PolicyConfig;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

/// Resolves client IPs to policy profile names.
///
/// Constructed from a [`PolicyConfig`] (parsed from `policy.toml`). The
/// resolver is cheap to clone (inner state is behind an `Arc`).
#[derive(Clone)]
pub struct ClientResolver {
    inner: Arc<Inner>,
}

struct Inner {
    /// Exact IP → profile name.
    exact: HashMap<IpAddr, String>,
    /// CIDR entries, checked in order.
    cidrs: Vec<CidrEntry>,
    /// The default profile name (always `"default"`).
    default_profile: String,
}

impl ClientResolver {
    /// Build a resolver from a parsed [`PolicyConfig`].
    pub fn from_config(config: &PolicyConfig) -> Self {
        let (exact, cidrs) = split_mappings(&config.clients);
        Self {
            inner: Arc::new(Inner {
                exact,
                cidrs,
                default_profile: config.default.name.clone(),
            }),
        }
    }

    /// Build a resolver with explicit mappings (useful for tests).
    pub fn new(
        exact: HashMap<IpAddr, String>,
        cidrs: Vec<CidrEntry>,
        default_profile: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                exact,
                cidrs,
                default_profile: default_profile.into(),
            }),
        }
    }

    /// Resolve a client IP to a profile name.
    ///
    /// Resolution order: exact IP → CIDR range → default.
    pub fn resolve(&self, ip: IpAddr) -> &str {
        // 1. Exact IP match.
        if let Some(profile) = self.inner.exact.get(&ip) {
            return profile.as_str();
        }
        // 2. CIDR range match.
        if let Some(profile) = find_matching_cidr(ip, &self.inner.cidrs) {
            return profile;
        }
        // 3. Default.
        &self.inner.default_profile
    }

    /// Returns the default profile name.
    pub fn default_profile(&self) -> &str {
        &self.inner.default_profile
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::IpAddr;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn test_static_ip_match() {
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.10"), "parents".to_string());
        let resolver = ClientResolver::new(exact, vec![], "default");
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "parents");
    }

    #[test]
    fn test_cidr_match() {
        let cidrs = vec![CidrEntry {
            cidr: "192.168.10.0/24".to_string(),
            profile: "guest".to_string(),
        }];
        let resolver = ClientResolver::new(HashMap::new(), cidrs, "default");
        assert_eq!(resolver.resolve(ip("192.168.10.50")), "guest");
    }

    #[test]
    fn test_default_fallback() {
        let resolver = ClientResolver::new(HashMap::new(), vec![], "default");
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");
    }

    #[test]
    fn test_exact_overrides_cidr() {
        // An exact IP mapping should win over a CIDR that also contains it.
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.10.5"), "special".to_string());
        let cidrs = vec![CidrEntry {
            cidr: "192.168.10.0/24".to_string(),
            profile: "guest".to_string(),
        }];
        let resolver = ClientResolver::new(exact, cidrs, "default");
        assert_eq!(resolver.resolve(ip("192.168.10.5")), "special");
        assert_eq!(resolver.resolve(ip("192.168.10.50")), "guest");
    }

    #[test]
    fn test_from_policy_config() {
        let toml = r#"
[default]
name = "default"
blocked_categories = ["ads"]

[clients."192.168.1.10"]
name = "dad"
policy = "parents"

[clients."192.168.10.0/24"]
name = "guest-net"
policy = "guest"
"#;
        let cfg = PolicyConfig::from_str(toml).unwrap();
        let resolver = ClientResolver::from_config(&cfg);
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "parents");
        assert_eq!(resolver.resolve(ip("192.168.10.50")), "guest");
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");
    }
}
