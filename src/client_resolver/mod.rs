//! Client IP → policy profile resolution (PRD section 4.5, lines 963-968).
//!
//! [`ClientResolver`] maps a client IP address to a policy profile name. The
//! resolution order (PRD lines 963-968) is:
//!
//! 1. **DHCP static lease** — MAC → IP → profile (explicit reservation in the
//!    DHCP lease table). Highest priority.
//! 2. **DHCP dynamic lease** — IP → hostname → profile via
//!    `[dhcp_integration].hostname_map` in `policy.toml`.
//! 3. **Static IP exact match** — `[clients."192.168.1.10"]` in `policy.toml`.
//! 4. **CIDR range match** — `[clients."192.168.10.0/24"]` in `policy.toml`.
//! 5. **Default profile** — the `[default]` profile name.
//!
//! Steps 1–2 consult the optional [`DhcpLookup`] (backed by the DHCPv4/v6
//! lease stores from stories 04-001 / 04-002). When DHCP is disabled or no
//! lease is found, the resolver falls through to static-IP / CIDR / default.

pub mod cidr;
pub mod config;
pub mod dhcp_lookup;
pub mod hostname_map;

pub use cidr::{find_matching_cidr, match_ip, CidrEntry};
pub use dhcp_lookup::{
    DhcpLeaseV4, DhcpLeaseV6, DhcpLookup, DhcpLookupResult, LeaseStoreV4, LeaseStoreV6,
};
pub use hostname_map::HostnameMap;

use crate::client_resolver::config::split_mappings;
use crate::policy::config::PolicyConfig;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;

/// Resolves client IPs to policy profile names.
///
/// Constructed from a [`PolicyConfig`] (parsed from `policy.toml`), optionally
/// with a [`DhcpLookup`] for DHCP lease table integration. The resolver is
/// cheap to clone (inner state is behind an `Arc`).
#[derive(Clone)]
pub struct ClientResolver {
    inner: Arc<Inner>,
}

struct Inner {
    /// DHCP lease lookup (v4 + v6). `None` when DHCP is disabled.
    dhcp: Option<DhcpLookup>,
    /// Hostname → profile map for dynamic lease resolution.
    hostname_map: HostnameMap,
    /// Exact IP → profile name.
    exact: HashMap<IpAddr, String>,
    /// CIDR entries, checked in order.
    cidrs: Vec<CidrEntry>,
    /// The default profile name (always `"default"`).
    default_profile: String,
}

impl ClientResolver {
    /// Build a resolver from a parsed [`PolicyConfig`].
    ///
    /// The hostname map is populated from `[dhcp_integration].hostname_map`.
    /// No DHCP lease stores are wired in — use [`Self::with_dhcp`] to add
    /// them once the lease stores are initialised.
    pub fn from_config(config: &PolicyConfig) -> Self {
        let (exact, cidrs) = split_mappings(&config.clients);
        let hostname_map = HostnameMap::new(config.dhcp_integration.hostname_map.clone());
        Self {
            inner: Arc::new(Inner {
                dhcp: None,
                hostname_map,
                exact,
                cidrs,
                default_profile: config.default.name.clone(),
            }),
        }
    }

    /// Build a resolver with explicit mappings (useful for tests).
    ///
    /// No DHCP lookup is configured; the resolver uses static-IP → CIDR →
    /// default. Use [`Self::with_dhcp`] to add DHCP lease lookup.
    pub fn new(
        exact: HashMap<IpAddr, String>,
        cidrs: Vec<CidrEntry>,
        default_profile: impl Into<String>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                dhcp: None,
                hostname_map: HostnameMap::empty(),
                exact,
                cidrs,
                default_profile: default_profile.into(),
            }),
        }
    }

    /// Build a resolver with explicit mappings and a hostname map (tests).
    ///
    /// No DHCP lease stores are wired in; the hostname map is still useful
    /// for tests that want to verify the dynamic-lease path via
    /// [`Self::with_dhcp`].
    pub fn with_hostname_map(
        exact: HashMap<IpAddr, String>,
        cidrs: Vec<CidrEntry>,
        default_profile: impl Into<String>,
        hostname_map: HostnameMap,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                dhcp: None,
                hostname_map,
                exact,
                cidrs,
                default_profile: default_profile.into(),
            }),
        }
    }

    /// Attach a [`DhcpLookup`] to this resolver, enabling DHCP lease table
    /// resolution (steps 1–2 of the resolution order).
    ///
    /// This is a builder-style method: it returns a new [`ClientResolver`]
    /// with the DHCP lookup wired in. The hostname map and static mappings
    /// are preserved. If the inner `Arc` has multiple owners (the resolver
    /// was cloned), the fields are cloned into a fresh `Arc`; otherwise the
    /// `Arc` is moved out and reused.
    pub fn with_dhcp(self, dhcp: DhcpLookup) -> Self {
        let old = Arc::try_unwrap(self.inner).unwrap_or_else(|arc| Inner {
            dhcp: None,
            hostname_map: arc.hostname_map.clone(),
            exact: arc.exact.clone(),
            cidrs: arc.cidrs.clone(),
            default_profile: arc.default_profile.clone(),
        });
        Self {
            inner: Arc::new(Inner {
                dhcp: Some(dhcp),
                ..old
            }),
        }
    }

    /// Resolve a client IP to a profile name.
    ///
    /// Resolution order (PRD lines 963-968):
    /// 1. DHCP static lease (MAC → IP → profile)
    /// 2. DHCP dynamic lease (IP → hostname → profile via hostname_map)
    /// 3. Static IP exact match
    /// 4. CIDR range match
    /// 5. Default profile
    pub fn resolve(&self, ip: IpAddr) -> &str {
        // 1–2. DHCP lease lookup (if configured).
        if let Some(dhcp) = &self.inner.dhcp {
            if let Some(result) = dhcp.lookup(ip) {
                match result {
                    DhcpLookupResult::Static { profile } => return profile,
                    DhcpLookupResult::Dynamic { hostname } => {
                        if let Some(profile) = self.inner.hostname_map.resolve(&hostname) {
                            return profile;
                        }
                        // Hostname not in the map — fall through to static-IP
                        // / CIDR / default.
                    }
                }
            }
        }
        // 3. Exact IP match.
        if let Some(profile) = self.inner.exact.get(&ip) {
            return profile.as_str();
        }
        // 4. CIDR range match.
        if let Some(profile) = find_matching_cidr(ip, &self.inner.cidrs) {
            return profile;
        }
        // 5. Default.
        &self.inner.default_profile
    }

    /// Returns the default profile name.
    pub fn default_profile(&self) -> &str {
        &self.inner.default_profile
    }

    /// Returns `true` if DHCP lease lookup is configured.
    pub fn has_dhcp(&self) -> bool {
        self.inner.dhcp.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_resolver::dhcp_lookup::{
        DhcpLeaseV4, DhcpLookup, LeaseStoreV4,
    };
    use std::collections::HashMap;
    use std::net::{IpAddr, Ipv4Addr};

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

    // ---- DHCP integration tests ----

    /// Minimal in-memory v4 lease store for resolver integration tests.
    struct MockLeaseStoreV4 {
        leases: HashMap<Ipv4Addr, DhcpLeaseV4>,
    }

    impl MockLeaseStoreV4 {
        fn new() -> Self {
            Self {
                leases: HashMap::new(),
            }
        }

        fn with(mut self, ip: Ipv4Addr, lease: DhcpLeaseV4) -> Self {
            self.leases.insert(ip, lease);
            self
        }
    }

    impl LeaseStoreV4 for MockLeaseStoreV4 {
        fn get_lease_by_ip(&self, ip: Ipv4Addr) -> Option<DhcpLeaseV4> {
            self.leases.get(&ip).cloned()
        }
    }

    fn lease_v4(ip: Ipv4Addr, hostname: Option<&str>, profile: Option<&str>) -> DhcpLeaseV4 {
        DhcpLeaseV4 {
            ip,
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: hostname.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
        }
    }

    fn hostname_map(pairs: &[(&str, &str)]) -> HostnameMap {
        let mut h = HashMap::new();
        for (k, v) in pairs {
            h.insert((*k).to_string(), (*v).to_string());
        }
        HostnameMap::new(h)
    }

    #[test]
    fn test_dhcp_static_lease_highest_priority() {
        // A DHCP static lease (profile = "iot") should win over a static IP
        // mapping (profile = "parents") for the same IP.
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.10"), "parents".to_string());
        let store = MockLeaseStoreV4::new().with(
            "192.168.1.10".parse().unwrap(),
            lease_v4("192.168.1.10".parse().unwrap(), Some("cam-01"), Some("iot")),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let resolver = ClientResolver::new(exact, vec![], "default").with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "iot");
        assert!(resolver.has_dhcp());
    }

    #[test]
    fn test_dhcp_dynamic_lease_via_hostname_map() {
        // A DHCP dynamic lease (hostname = "kids-tablet") resolves to "kids"
        // via the hostname map, before falling back to static IP / CIDR.
        let store = MockLeaseStoreV4::new().with(
            "192.168.1.20".parse().unwrap(),
            lease_v4("192.168.1.20".parse().unwrap(), Some("kids-tablet"), None),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let hmap = hostname_map(&[("kids-tablet", "kids"), ("dad-laptop", "parents")]);
        let resolver =
            ClientResolver::with_hostname_map(HashMap::new(), vec![], "default", hmap)
                .with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.20")), "kids");
    }

    #[test]
    fn test_dhcp_dynamic_lease_hostname_not_in_map_falls_through() {
        // If the hostname is not in the hostname_map, the resolver falls
        // through to static-IP / CIDR / default.
        let store = MockLeaseStoreV4::new().with(
            "192.168.1.20".parse().unwrap(),
            lease_v4("192.168.1.20".parse().unwrap(), Some("unknown-device"), None),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let hmap = hostname_map(&[("kids-tablet", "kids")]);
        let resolver =
            ClientResolver::with_hostname_map(HashMap::new(), vec![], "default", hmap)
                .with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.20")), "default");
    }

    #[test]
    fn test_dhcp_dynamic_falls_through_to_static_ip() {
        // Dynamic lease hostname not in map → fall through to static IP.
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.30"), "parents".to_string());
        let store = MockLeaseStoreV4::new().with(
            "192.168.1.30".parse().unwrap(),
            lease_v4("192.168.1.30".parse().unwrap(), Some("unknown"), None),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let resolver = ClientResolver::new(exact, vec![], "default").with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.30")), "parents");
    }

    #[test]
    fn test_dhcp_dynamic_falls_through_to_cidr() {
        // Dynamic lease hostname not in map → fall through to CIDR.
        let cidrs = vec![CidrEntry {
            cidr: "192.168.10.0/24".to_string(),
            profile: "guest".to_string(),
        }];
        let store = MockLeaseStoreV4::new().with(
            "192.168.10.50".parse().unwrap(),
            lease_v4("192.168.10.50".parse().unwrap(), Some("unknown"), None),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let resolver = ClientResolver::new(HashMap::new(), cidrs, "default").with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.10.50")), "guest");
    }

    #[test]
    fn test_no_dhcp_falls_through_to_static_ip() {
        // When DHCP is not configured, the resolver uses static-IP → CIDR →
        // default (the original 02-001 behaviour).
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.10"), "parents".to_string());
        let resolver = ClientResolver::new(exact, vec![], "default");
        assert!(!resolver.has_dhcp());
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "parents");
    }

    #[test]
    fn test_dhcp_no_lease_falls_through() {
        // DHCP is configured but no lease exists for the IP → fall through.
        let store = MockLeaseStoreV4::new();
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.10"), "parents".to_string());
        let resolver = ClientResolver::new(exact, vec![], "default").with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "parents");
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");
    }

    #[test]
    fn test_full_resolution_order() {
        // Verify the complete priority order with all paths configured:
        // 1. DHCP static lease
        // 2. DHCP dynamic lease → hostname_map
        // 3. Static IP
        // 4. CIDR
        // 5. Default
        let mut exact = HashMap::new();
        exact.insert(ip("192.168.1.10"), "static-parents".to_string());
        exact.insert(ip("192.168.1.40"), "static-ip".to_string());

        let cidrs = vec![CidrEntry {
            cidr: "192.168.20.0/24".to_string(),
            profile: "cidr-iot".to_string(),
        }];

        let hmap = hostname_map(&[("kids-tablet", "dynamic-kids")]);

        let store = MockLeaseStoreV4::new()
            // 1. DHCP static lease wins over static IP mapping.
            .with(
                "192.168.1.10".parse().unwrap(),
                lease_v4("192.168.1.10".parse().unwrap(), Some("cam"), Some("dhcp-static-iot")),
            )
            // 2. DHCP dynamic lease → hostname_map.
            .with(
                "192.168.1.20".parse().unwrap(),
                lease_v4("192.168.1.20".parse().unwrap(), Some("kids-tablet"), None),
            )
            // 3. Static IP (no lease for this IP).
            // 192.168.1.40 has a static mapping but no lease.
            // 4. CIDR (no lease for this IP).
            // 192.168.20.5 has a CIDR match but no lease.
            // 5. Default (no lease, no static, no CIDR).
            // 10.0.0.1 → default
            ;

        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let resolver =
            ClientResolver::with_hostname_map(exact, cidrs, "default", hmap).with_dhcp(dhcp);

        // 1. DHCP static lease.
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "dhcp-static-iot");
        // 2. DHCP dynamic lease → hostname_map.
        assert_eq!(resolver.resolve(ip("192.168.1.20")), "dynamic-kids");
        // 3. Static IP.
        assert_eq!(resolver.resolve(ip("192.168.1.40")), "static-ip");
        // 4. CIDR.
        assert_eq!(resolver.resolve(ip("192.168.20.5")), "cidr-iot");
        // 5. Default.
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");
    }

    #[test]
    fn test_from_config_with_dhcp_integration() {
        // Verify that from_config parses the hostname_map and that
        // with_dhcp wires it together.
        let toml = r#"
[default]
name = "default"
blocked_categories = ["ads"]

[clients."192.168.1.10"]
name = "dad"
policy = "parents"

[dhcp_integration]
hostname_map = { "kids-tablet" = "kids", "dad-laptop" = "parents" }
"#;
        let cfg = PolicyConfig::from_str(toml).unwrap();
        let resolver = ClientResolver::from_config(&cfg);
        // Without DHCP, static IP works.
        assert_eq!(resolver.resolve(ip("192.168.1.10")), "parents");
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");

        // With DHCP dynamic lease for "kids-tablet" → "kids".
        let store = MockLeaseStoreV4::new().with(
            "192.168.1.50".parse().unwrap(),
            lease_v4("192.168.1.50".parse().unwrap(), Some("kids-tablet"), None),
        );
        let dhcp = DhcpLookup::new(Some(std::sync::Arc::new(store)), None);
        let resolver = resolver.with_dhcp(dhcp);
        assert_eq!(resolver.resolve(ip("192.168.1.50")), "kids");
    }

    #[test]
    fn test_dhcp_disabled_graceful() {
        // DhcpLookup::disabled() returns None for all lookups; the resolver
        // falls straight through to static-IP / CIDR / default.
        let dhcp = DhcpLookup::disabled();
        let resolver = ClientResolver::new(HashMap::new(), vec![], "default").with_dhcp(dhcp);
        assert!(resolver.has_dhcp());
        assert_eq!(resolver.resolve(ip("10.0.0.1")), "default");
    }
}
