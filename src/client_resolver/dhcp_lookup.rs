//! DHCP lease table lookup for client policy resolution
//! (PRD section 4.5, lines 957-968).
//!
//! [`DhcpLookup`] queries the DHCPv4 and DHCPv6 lease stores for a given
//! client IP. The lease stores are supplied as trait objects
//! ([`LeaseStoreV4`] / [`LeaseStoreV6`]) so that the concrete SQLite-backed
//! implementations from stories 04-001 / 04-002 can be plugged in without the
//! client resolver depending on their concrete types.
//!
//! Resolution order (PRD lines 963-968):
//!
//! 1. **DHCP static lease** — MAC → IP → profile (explicit reservation). The
//!    lease record carries a `profile` directly.
//! 2. **DHCP dynamic lease** — IP → hostname. The hostname is resolved to a
//!    profile via [`crate::client_resolver::hostname_map::HostnameMap`].
//!
//! Both paths are surfaced to the caller as a [`DhcpLookupResult`]; the
//! [`crate::client_resolver::ClientResolver`] decides how to combine the
//! result with the hostname map and the static-IP/CIDR fallbacks.

use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;

/// A DHCPv4 lease record, as seen by the client resolver.
///
/// Only the fields needed for policy resolution are exposed. The concrete
/// lease store (story 04-001) holds additional metadata (expiry, lease
/// state, etc.) that is not relevant here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpLeaseV4 {
    /// The leased IPv4 address.
    pub ip: Ipv4Addr,
    /// Client MAC address (canonical form, e.g. `"aa:bb:cc:dd:ee:ff"`).
    pub mac: String,
    /// Client hostname, if the lease carries one (DHCP option 12).
    pub hostname: Option<String>,
    /// Explicit profile name for static reservations (MAC → IP → profile).
    ///
    /// `None` for dynamic leases — the profile is derived from the hostname
    /// via the hostname map.
    pub profile: Option<String>,
}

/// A DHCPv6 lease record, as seen by the client resolver.
///
/// Mirrors [`DhcpLeaseV4`] for the IPv6 lease store (story 04-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DhcpLeaseV6 {
    /// The leased IPv6 address.
    pub ip: Ipv6Addr,
    /// Client DUID (DHCP Unique IDentifier), analogous to a MAC in v4.
    pub duid: String,
    /// Client hostname, if the lease carries one.
    pub hostname: Option<String>,
    /// Explicit profile name for static reservations.
    pub profile: Option<String>,
}

/// Trait abstracting a DHCPv4 lease store's lookup-by-IP operation.
///
/// The concrete implementation (story 04-001, `SqliteLeaseStoreV4`) will
/// implement this trait so that [`DhcpLookup`] can query it without a hard
/// dependency on the `dhcp` module.
pub trait LeaseStoreV4: Send + Sync {
    /// Return the active lease for `ip`, if any.
    fn get_lease_by_ip(&self, ip: Ipv4Addr) -> Option<DhcpLeaseV4>;
}

/// Trait abstracting a DHCPv6 lease store's lookup-by-IP operation.
///
/// The concrete implementation (story 04-002, `SqliteLeaseStoreV6`) will
/// implement this trait.
pub trait LeaseStoreV6: Send + Sync {
    /// Return the active lease for `ip`, if any.
    fn get_lease_by_ip(&self, ip: Ipv6Addr) -> Option<DhcpLeaseV6>;
}

/// The outcome of a DHCP lease lookup for a client IP.
///
/// The [`crate::client_resolver::ClientResolver`] uses this to distinguish
/// the two DHCP resolution paths (static vs. dynamic) before falling back to
/// static-IP / CIDR / default matching.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DhcpLookupResult {
    /// A static reservation with an explicit profile (MAC → IP → profile).
    ///
    /// This is the highest-priority resolution path (PRD line 964). The
    /// `profile` is an interned `&'static str` — see [`DhcpLookup`] for
    /// details on the interning strategy.
    Static {
        /// The profile name assigned to the reservation.
        profile: &'static str,
    },
    /// A dynamic lease carrying a hostname (IP → hostname).
    ///
    /// The profile is derived from the hostname via the hostname map
    /// (PRD line 965). If the hostname is not in the map, the resolver
    /// falls through to static-IP / CIDR / default.
    Dynamic {
        /// The client hostname from the lease (DHCP option 12).
        hostname: String,
    },
}

impl DhcpLookupResult {
    /// The profile for a static lease, if this is a static result.
    pub fn static_profile(&self) -> Option<&'static str> {
        match self {
            DhcpLookupResult::Static { profile } => Some(*profile),
            DhcpLookupResult::Dynamic { .. } => None,
        }
    }

    /// The hostname for a dynamic lease, if this is a dynamic result.
    pub fn dynamic_hostname(&self) -> Option<&str> {
        match self {
            DhcpLookupResult::Dynamic { hostname } => Some(hostname.as_str()),
            DhcpLookupResult::Static { .. } => None,
        }
    }
}

/// A string interner that deduplicates `Box::leak` calls for DHCP static
/// profile names.
///
/// DHCP static reservations carry an explicit profile name (e.g. `"parents"`).
/// To return a `&'static str` from [`DhcpLookup`] (so that
/// [`crate::client_resolver::ClientResolver::resolve`] can return `&str`
/// without changing its public API), profile strings are interned here: the
/// first time a profile name is seen it is `Box::leak`-ed into a `&'static
/// str`; subsequent lookups return the same reference.
///
/// The number of distinct profile names is small and bounded (one per static
/// reservation, typically a handful), so the leaked memory is negligible. The
/// interner is shared across all clones of [`DhcpLookup`] via `Arc`.
type Interner = Arc<Mutex<HashMap<String, &'static str>>>;

fn intern(interner: &Interner, s: &str) -> &'static str {
    let mut guard = interner.lock();
    if let Some(existing) = guard.get(s) {
        return *existing;
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    guard.insert(s.to_string(), leaked);
    leaked
}

/// Queries DHCPv4 and DHCPv6 lease stores to resolve a client IP to a
/// hostname and/or profile.
///
/// Holds optional references to the v4 and v6 lease stores. Either may be
/// `None` when DHCP is disabled for that family — the lookup simply returns
/// `None` and the resolver falls through to static-IP / CIDR / default.
///
/// Static-lease profile names are interned (see [`Interner`]) so that
/// [`DhcpLookupResult::Static`] can carry a `&'static str`, allowing
/// [`crate::client_resolver::ClientResolver::resolve`] to return `&str`
/// without changing its public API. The interner is shared across all clones
/// of `DhcpLookup` via `Arc`.
///
/// `DhcpLookup` is cheap to clone (lease-store references and interner are
/// behind `Arc`).
#[derive(Clone)]
pub struct DhcpLookup {
    v4: Option<Arc<dyn LeaseStoreV4>>,
    v6: Option<Arc<dyn LeaseStoreV6>>,
    interner: Interner,
}

impl DhcpLookup {
    /// Build a `DhcpLookup` from optional v4/v6 lease stores.
    ///
    /// Pass `None` for a family when DHCP is disabled or the lease store is
    /// not yet initialised.
    pub fn new(
        v4: Option<Arc<dyn LeaseStoreV4>>,
        v6: Option<Arc<dyn LeaseStoreV6>>,
    ) -> Self {
        Self {
            v4,
            v6,
            interner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Build an empty `DhcpLookup` with no lease stores (DHCP disabled).
    ///
    /// All lookups will return `None`; the resolver falls straight through to
    /// static-IP / CIDR / default.
    pub fn disabled() -> Self {
        Self::new(None, None)
    }

    /// Query the lease stores for `ip`.
    ///
    /// Returns `Some(Static { profile })` for a static reservation with an
    /// explicit profile, `Some(Dynamic { hostname })` for a dynamic lease
    /// with a hostname, or `None` when there is no lease or the lease carries
    /// neither a profile nor a hostname.
    pub fn lookup(&self, ip: IpAddr) -> Option<DhcpLookupResult> {
        match ip {
            IpAddr::V4(v4) => self.query_v4(v4),
            IpAddr::V6(v6) => self.query_v6(v6),
        }
    }

    /// Query the DHCPv4 lease store for `ip`.
    pub fn query_v4(&self, ip: Ipv4Addr) -> Option<DhcpLookupResult> {
        let store = self.v4.as_ref()?;
        let lease = store.get_lease_by_ip(ip)?;
        lease_to_result(&self.interner, lease.profile.as_ref(), lease.hostname.as_ref())
    }

    /// Query the DHCPv6 lease store for `ip`.
    pub fn query_v6(&self, ip: Ipv6Addr) -> Option<DhcpLookupResult> {
        let store = self.v6.as_ref()?;
        let lease = store.get_lease_by_ip(ip)?;
        lease_to_result(&self.interner, lease.profile.as_ref(), lease.hostname.as_ref())
    }
}

/// Convert a lease's (profile, hostname) pair into a [`DhcpLookupResult`].
///
/// - If the lease has an explicit `profile` (static reservation), the profile
///   is interned and returned as `Static`.
/// - Otherwise, if the lease has a `hostname`, return `Dynamic`.
/// - Otherwise, return `None` (the lease exists but carries no usable
///   identity — the resolver falls through).
fn lease_to_result(
    interner: &Interner,
    profile: Option<&String>,
    hostname: Option<&String>,
) -> Option<DhcpLookupResult> {
    if let Some(p) = profile {
        let p = p.trim();
        if !p.is_empty() {
            return Some(DhcpLookupResult::Static {
                profile: intern(interner, p),
            });
        }
    }
    if let Some(h) = hostname {
        let h = h.trim();
        if !h.is_empty() {
            return Some(DhcpLookupResult::Dynamic {
                hostname: h.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::Ipv4Addr;

    fn v4(s: &str) -> Ipv4Addr {
        s.parse().unwrap()
    }

    /// A minimal in-memory v4 lease store for testing.
    struct MockStoreV4 {
        leases: HashMap<Ipv4Addr, DhcpLeaseV4>,
    }

    impl MockStoreV4 {
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

    impl LeaseStoreV4 for MockStoreV4 {
        fn get_lease_by_ip(&self, ip: Ipv4Addr) -> Option<DhcpLeaseV4> {
            self.leases.get(&ip).cloned()
        }
    }

    fn make_lease(ip: Ipv4Addr, hostname: Option<&str>, profile: Option<&str>) -> DhcpLeaseV4 {
        DhcpLeaseV4 {
            ip,
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            hostname: hostname.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_query_v4_static_lease() {
        let store = MockStoreV4::new().with(
            v4("192.168.1.50"),
            make_lease(v4("192.168.1.50"), Some("dad-laptop"), Some("parents")),
        );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        let result = lookup.query_v4(v4("192.168.1.50")).unwrap();
        assert_eq!(result.static_profile(), Some("parents"));
        assert_eq!(result.dynamic_hostname(), None);
    }

    #[test]
    fn test_query_v4_dynamic_lease() {
        let store = MockStoreV4::new().with(
            v4("192.168.1.51"),
            make_lease(v4("192.168.1.51"), Some("kids-tablet"), None),
        );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        let result = lookup.query_v4(v4("192.168.1.51")).unwrap();
        assert_eq!(result.dynamic_hostname(), Some("kids-tablet"));
        assert_eq!(result.static_profile(), None);
    }

    #[test]
    fn test_query_v4_not_found() {
        let store = MockStoreV4::new();
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        assert_eq!(lookup.query_v4(v4("10.0.0.1")), None);
    }

    #[test]
    fn test_query_v4_no_store() {
        let lookup = DhcpLookup::disabled();
        assert_eq!(lookup.query_v4(v4("192.168.1.50")), None);
    }

    #[test]
    fn test_query_v4_lease_without_hostname_or_profile() {
        // A lease with neither a profile nor a hostname is not useful for
        // policy resolution — the lookup returns None so the resolver can
        // fall through to static-IP / CIDR / default.
        let store =
            MockStoreV4::new().with(v4("192.168.1.52"), make_lease(v4("192.168.1.52"), None, None));
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        assert_eq!(lookup.query_v4(v4("192.168.1.52")), None);
    }

    #[test]
    fn test_query_v4_static_overrides_dynamic() {
        // A static reservation with both a profile and a hostname should
        // report as Static (profile wins over hostname).
        let store = MockStoreV4::new().with(
            v4("192.168.1.53"),
            make_lease(v4("192.168.1.53"), Some("dad-laptop"), Some("parents")),
        );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        let result = lookup.query_v4(v4("192.168.1.53")).unwrap();
        assert_eq!(result.static_profile(), Some("parents"));
    }

    #[test]
    fn test_lookup_dispatches_by_ip_family() {
        let store = MockStoreV4::new().with(
            v4("192.168.1.54"),
            make_lease(v4("192.168.1.54"), None, Some("iot")),
        );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        // IPv4 dispatches to query_v4.
        let result = lookup.lookup(IpAddr::V4(v4("192.168.1.54"))).unwrap();
        assert_eq!(result.static_profile(), Some("iot"));
        // IPv6 with no v6 store returns None.
        let result = lookup.lookup(IpAddr::V6("2001:db8::1".parse().unwrap()));
        assert_eq!(result, None);
    }

    #[test]
    fn test_empty_strings_treated_as_absent() {
        // A lease with empty-string profile/hostname should not match.
        let store = MockStoreV4::new().with(
            v4("192.168.1.55"),
            make_lease(v4("192.168.1.55"), Some("  "), Some("")),
        );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        assert_eq!(lookup.query_v4(v4("192.168.1.55")), None);
    }

    #[test]
    fn test_interner_deduplicates() {
        // The same profile string queried twice should return the same
        // &'static str pointer (deduplication via the interner).
        let store = MockStoreV4::new()
            .with(
                v4("192.168.1.60"),
                make_lease(v4("192.168.1.60"), None, Some("parents")),
            )
            .with(
                v4("192.168.1.61"),
                make_lease(v4("192.168.1.61"), None, Some("parents")),
            );
        let lookup = DhcpLookup::new(Some(Arc::new(store)), None);
        let r1 = lookup.query_v4(v4("192.168.1.60")).unwrap();
        let r2 = lookup.query_v4(v4("192.168.1.61")).unwrap();
        let p1 = r1.static_profile().unwrap();
        let p2 = r2.static_profile().unwrap();
        assert_eq!(p1, "parents");
        assert_eq!(p2, "parents");
        // Same interned pointer (deduplication).
        assert!(std::ptr::eq(p1.as_ptr(), p2.as_ptr()));
    }

    // ---- DHCPv6 tests ----

    struct MockStoreV6 {
        leases: HashMap<Ipv6Addr, DhcpLeaseV6>,
    }

    impl MockStoreV6 {
        fn new() -> Self {
            Self {
                leases: HashMap::new(),
            }
        }

        fn with(mut self, ip: Ipv6Addr, lease: DhcpLeaseV6) -> Self {
            self.leases.insert(ip, lease);
            self
        }
    }

    impl LeaseStoreV6 for MockStoreV6 {
        fn get_lease_by_ip(&self, ip: Ipv6Addr) -> Option<DhcpLeaseV6> {
            self.leases.get(&ip).cloned()
        }
    }

    fn v6(s: &str) -> Ipv6Addr {
        s.parse().unwrap()
    }

    fn make_lease_v6(ip: Ipv6Addr, hostname: Option<&str>, profile: Option<&str>) -> DhcpLeaseV6 {
        DhcpLeaseV6 {
            ip,
            duid: "0001:0002:0003".to_string(),
            hostname: hostname.map(|s| s.to_string()),
            profile: profile.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_query_v6_static_lease() {
        let store = MockStoreV6::new().with(
            v6("2001:db8::10"),
            make_lease_v6(v6("2001:db8::10"), Some("dad-laptop"), Some("parents")),
        );
        let lookup = DhcpLookup::new(None, Some(Arc::new(store)));
        let result = lookup.query_v6(v6("2001:db8::10")).unwrap();
        assert_eq!(result.static_profile(), Some("parents"));
    }

    #[test]
    fn test_query_v6_dynamic_lease() {
        let store = MockStoreV6::new().with(
            v6("2001:db8::11"),
            make_lease_v6(v6("2001:db8::11"), Some("kids-tablet"), None),
        );
        let lookup = DhcpLookup::new(None, Some(Arc::new(store)));
        let result = lookup.query_v6(v6("2001:db8::11")).unwrap();
        assert_eq!(result.dynamic_hostname(), Some("kids-tablet"));
    }

    #[test]
    fn test_query_v6_not_found() {
        let store = MockStoreV6::new();
        let lookup = DhcpLookup::new(None, Some(Arc::new(store)));
        assert_eq!(lookup.query_v6(v6("2001:db8::99")), None);
    }

    #[test]
    fn test_lookup_result_accessors() {
        // Build a Static result via the interner to get a real &'static str.
        let interner: Interner = Arc::new(Mutex::new(HashMap::new()));
        let s = DhcpLookupResult::Static {
            profile: intern(&interner, "p"),
        };
        assert_eq!(s.static_profile(), Some("p"));
        assert_eq!(s.dynamic_hostname(), None);

        let d = DhcpLookupResult::Dynamic {
            hostname: "h".to_string(),
        };
        assert_eq!(d.static_profile(), None);
        assert_eq!(d.dynamic_hostname(), Some("h"));
    }
}
