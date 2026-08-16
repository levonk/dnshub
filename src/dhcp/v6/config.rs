//! DHCPv6 server configuration (`[dhcp.v6]` and `[[dhcp.v6.pools]]`).
//!
//! Mirrors PRD section 4.4 (lines 832-855). The configuration is a
//! self-contained struct living under [`crate::dhcp::v6`] so the DHCPv6
//! server module owns its own shape. The top-level [`crate::config::DhcpConfig`]
//! stub from story 01-004 is intentionally left untouched; a later wiring
//! story will embed this struct into the global config tree.

use serde::{Deserialize, Serialize};
use std::net::Ipv6Addr;

/// `[dhcp.v6]` — DHCPv6 server configuration (PRD lines 832-855).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpV6Config {
    /// Whether the DHCPv6 server is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// UDP listen address for the DHCPv6 server, e.g. `"[::]:547"`.
    #[serde(default = "default_listen")]
    pub listen: String,

    /// Domain search list sent in option 24.
    #[serde(default)]
    pub domain_search: Vec<String>,

    /// Default lease duration in hours (maps to IA_NA valid lifetime).
    #[serde(default = "default_lease_time_hours")]
    pub lease_time_hours: u32,

    /// NTP server address sent in option 56.
    #[serde(default)]
    pub ntp_server: Option<Ipv6Addr>,

    /// Address pools the server allocates from.
    #[serde(default)]
    pub pools: Vec<DhcpPoolV6>,
}

impl Default for DhcpV6Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_listen(),
            domain_search: Vec::new(),
            lease_time_hours: default_lease_time_hours(),
            ntp_server: None,
            pools: Vec::new(),
        }
    }
}

fn default_listen() -> String {
    "[::]:547".to_string()
}

fn default_lease_time_hours() -> u32 {
    24
}

impl DhcpV6Config {
    /// Default valid lifetime in seconds (`lease_time_hours * 3600`).
    pub fn valid_lifetime_secs(&self) -> u32 {
        self.lease_time_hours.saturating_mul(3600)
    }

    /// Default preferred lifetime in seconds (half the valid lifetime,
    /// per RFC 8415 §12 recommended T1 = 0.5 * valid).
    pub fn preferred_lifetime_secs(&self) -> u32 {
        self.valid_lifetime_secs() / 2
    }

    /// T1 (renew) timer in seconds. Defaults to 0.5 * valid lifetime.
    pub fn t1_secs(&self) -> u32 {
        self.valid_lifetime_secs() / 2
    }

    /// T2 (rebind) timer in seconds. Defaults to 0.8 * valid lifetime.
    pub fn t2_secs(&self) -> u32 {
        self.valid_lifetime_secs() * 4 / 5
    }

    /// Information refresh time (option 32) in seconds for stateless
    /// INFORMATION-REQUEST replies. Defaults to the valid lifetime.
    pub fn info_refresh_time_secs(&self) -> u32 {
        self.valid_lifetime_secs()
    }

    /// Look up a pool by name.
    pub fn pool(&self, name: &str) -> Option<&DhcpPoolV6> {
        self.pools.iter().find(|p| p.name == name)
    }
}

/// `[[dhcp.v6.pools]]` — a single DHCPv6 address pool (PRD lines 850-855).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DhcpPoolV6 {
    /// Human-readable pool name, e.g. `"main"`.
    pub name: String,
    /// Pool prefix in CIDR notation, e.g. `"fd00:1234:5678::/64"`.
    pub prefix: String,
    /// First address in the dynamic allocation range.
    pub pool_start: Ipv6Addr,
    /// Last address in the dynamic allocation range.
    pub pool_end: Ipv6Addr,
    /// DNS server addresses advertised for this pool (option 23).
    #[serde(default)]
    pub dns_servers: Vec<Ipv6Addr>,
}

impl DhcpPoolV6 {
    /// Iterate over every address in the `[pool_start, pool_end]` range
    /// inclusive. Used by the allocator to scan for a free address.
    pub fn iter_addresses(&self) -> impl Iterator<Item = Ipv6Addr> {
        let start = u128::from(self.pool_start);
        let end = u128::from(self.pool_end);
        (start..=end).map(|n| Ipv6Addr::from(n))
    }

    /// Number of addresses in the pool range.
    pub fn size(&self) -> u128 {
        let start = u128::from(self.pool_start);
        let end = u128::from(self.pool_end);
        if end < start {
            return 0;
        }
        end - start + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let cfg = DhcpV6Config::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.listen, "[::]:547");
        assert_eq!(cfg.lease_time_hours, 24);
        assert_eq!(cfg.valid_lifetime_secs(), 86_400);
        assert_eq!(cfg.preferred_lifetime_secs(), 43_200);
        assert_eq!(cfg.t1_secs(), 43_200);
        assert_eq!(cfg.t2_secs(), 69_120);
    }

    #[test]
    fn pool_iter_and_size() {
        let pool = DhcpPoolV6 {
            name: "main".into(),
            prefix: "fd00:1234:5678::/64".into(),
            pool_start: "fd00:1234:5678::100".parse().unwrap(),
            pool_end: "fd00:1234:5678::103".parse().unwrap(),
            dns_servers: vec!["fd00:1234:5678::67".parse().unwrap()],
        };
        let addrs: Vec<_> = pool.iter_addresses().collect();
        assert_eq!(addrs.len(), 4);
        assert_eq!(pool.size(), 4);
        assert_eq!(addrs[0], "fd00:1234:5678::100".parse::<Ipv6Addr>().unwrap());
        assert_eq!(addrs[3], "fd00:1234:5678::103".parse::<Ipv6Addr>().unwrap());
    }

    #[test]
    fn deserialize_from_toml() {
        let toml = r#"
enabled = true
listen = "[::]:547"
domain_search = ["levonk.com"]
lease_time_hours = 12
ntp_server = "fd00:1234:5678::55"

[[pools]]
name = "main"
prefix = "fd00:1234:5678::/64"
pool_start = "fd00:1234:5678::100"
pool_end = "fd00:1234:5678::1ff"
dns_servers = ["fd00:1234:5678::67"]
"#;
        let cfg: DhcpV6Config = toml::from_str(toml).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.lease_time_hours, 12);
        assert_eq!(cfg.domain_search, vec!["levonk.com"]);
        assert_eq!(
            cfg.ntp_server,
            Some("fd00:1234:5678::55".parse().unwrap())
        );
        assert_eq!(cfg.pools.len(), 1);
        assert_eq!(cfg.pool("main").unwrap().size(), 256);
    }
}
